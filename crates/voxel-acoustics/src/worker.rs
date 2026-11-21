use std::sync::{
    Arc, Condvar, Mutex, RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

use glam::Vec3;
use voxel_core::VoxelWorld;

use crate::{
    pathtracer::PathTraceConfig,
    solver::{AcousticSnapshot, AcousticSourceQuery, solve_acoustics},
};

/// Input payload for an asynchronous acoustic solve executed by [`AcousticWorker`].
#[derive(Clone, Debug)]
pub struct AcousticWorkRequest {
    pub listener_pos_m: Vec3,
    pub listener_forward: Vec3,
    pub listener_right: Vec3,
    pub sources: Vec<AcousticSourceQuery>,
    pub cfg: PathTraceConfig,
    pub frame_seed: u64,
}

struct WorkerShared {
    pending: Mutex<Option<AcousticWorkRequest>>,
    wakeup: Condvar,
    latest_snapshot: Mutex<Option<AcousticSnapshot>>,
    shutdown: AtomicBool,
}

/// Dedicated background OS thread that solves voxel acoustics off the main frame thread.
///
/// Uses a latest-only mailbox so rapid submissions overwrite stale pending requests instead
/// of backing up an unbounded queue.
pub struct AcousticWorker {
    shared: Arc<WorkerShared>,
    thread: Option<JoinHandle<()>>,
}

impl AcousticWorker {
    /// Spawns a dedicated background worker thread named `"voxel-acoustics-worker"` that
    /// acquires short read locks on `world` during each acoustic solve.
    pub fn spawn(world: Arc<RwLock<VoxelWorld>>) -> Self {
        let shared = Arc::new(WorkerShared {
            pending: Mutex::new(None),
            wakeup: Condvar::new(),
            latest_snapshot: Mutex::new(None),
            shutdown: AtomicBool::new(false),
        });

        let worker_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("voxel-acoustics-worker".to_string())
            .spawn(move || worker_loop(world, worker_shared))
            .expect("failed to spawn voxel-acoustics-worker thread");

        Self {
            shared,
            thread: Some(thread),
        }
    }

    /// Submits a new acoustic solve request without blocking on solve execution.
    ///
    /// If a previous request has not yet been picked up by the worker, it is overwritten
    /// so the worker always solves the freshest listener/source state.
    pub fn submit(&self, req: AcousticWorkRequest) {
        {
            let mut pending = self
                .shared
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *pending = Some(req);
        }
        self.shared.wakeup.notify_one();
    }

    /// Non-blocking poll for the most recently completed [`AcousticSnapshot`].
    pub fn try_take_snapshot(&self) -> Option<AcousticSnapshot> {
        self.shared
            .latest_snapshot
            .try_lock()
            .ok()
            .and_then(|mut guard| guard.take())
    }
}

impl Drop for AcousticWorker {
    fn drop(&mut self) {
        {
            // Set shutdown while holding the mailbox mutex so the worker can never miss
            // a wakeup between checking `shutdown` and entering `Condvar::wait`.
            let _guard = self
                .shared
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            self.shared.shutdown.store(true, Ordering::Release);
        }
        self.shared.wakeup.notify_all();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn worker_loop(world: Arc<RwLock<VoxelWorld>>, shared: Arc<WorkerShared>) {
    loop {
        let req = {
            let mut pending = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if shared.shutdown.load(Ordering::Acquire) {
                    break None;
                }
                if let Some(req) = pending.take() {
                    break Some(req);
                }
                pending = shared
                    .wakeup
                    .wait(pending)
                    .unwrap_or_else(|e| e.into_inner());
            }
        };

        let Some(req) = req else {
            break;
        };

        let snapshot = {
            let world_guard = world.read().unwrap_or_else(|e| e.into_inner());
            solve_acoustics(
                &world_guard,
                req.listener_pos_m,
                req.listener_forward,
                req.listener_right,
                &req.sources,
                req.cfg,
                req.frame_seed,
            )
        };

        let mut latest = shared
            .latest_snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *latest = Some(snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use glam::{IVec3, UVec3};
    use voxel_core::{VoxelConfig, VoxelId};

    #[test]
    fn worker_solves_request_asynchronously_and_shuts_down_cleanly() {
        let vcfg = VoxelConfig::half_player_scale(UVec3::splat(2));
        let mut world = VoxelWorld::new(vcfg);

        // Build an enclosed stone room so reverb & reflections are non-trivial
        let start = IVec3::splat(4);
        let end = IVec3::splat(20);
        for y in start.y..=end.y {
            for z in start.z..=end.z {
                for x in start.x..=end.x {
                    let shell = x == start.x
                        || x == end.x
                        || y == start.y
                        || y == end.y
                        || z == start.z
                        || z == end.z;
                    if shell {
                        world.set(IVec3::new(x, y, z), VoxelId::CaveStone);
                    }
                }
            }
        }

        let listener_pos = vcfg.voxel_to_world_center(IVec3::new(10, 10, 10));
        let source_pos = vcfg.voxel_to_world_center(IVec3::new(15, 10, 10));

        let shared_world = Arc::new(RwLock::new(world));
        let worker = AcousticWorker::spawn(Arc::clone(&shared_world));

        assert!(worker.try_take_snapshot().is_none());

        worker.submit(AcousticWorkRequest {
            listener_pos_m: listener_pos,
            listener_forward: Vec3::X,
            listener_right: Vec3::Z,
            sources: vec![AcousticSourceQuery {
                id: 42,
                pos_m: source_pos,
                ref_distance_m: 2.0,
                max_distance_m: 60.0,
            }],
            cfg: PathTraceConfig {
                reverb_rays: 32,
                early_reflection_rays: 16,
                ..Default::default()
            },
            frame_seed: 7,
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        let snapshot = loop {
            if let Some(snap) = worker.try_take_snapshot() {
                break snap;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for AcousticWorker snapshot"
            );
            thread::sleep(Duration::from_millis(2));
        };

        assert_eq!(snapshot.listener_pos_m, listener_pos);
        assert_eq!(snapshot.listener_forward, Vec3::X);
        assert_eq!(snapshot.listener_right, Vec3::Z);
        assert!(snapshot.reverb.enclosure > 0.8);
        assert!(snapshot.reverb.rt60_sec.mean() > 0.15);
        assert_eq!(snapshot.sources.len(), 1);
        assert_eq!(snapshot.sources[0].id, 42);
        assert!(snapshot.sources[0].direct_gain.mean() > 0.0);
        assert!(snapshot.sources[0].reverb_send > 0.0);

        // Snapshot should be consumed once taken
        assert!(worker.try_take_snapshot().is_none());

        // Explicit drop verifies clean thread shutdown without deadlock
        drop(worker);
    }
}
