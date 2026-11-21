//! Synchronous Bevy-friendly P2P session handle backed by a dedicated Tokio runtime
//! and `iroh` QUIC endpoint.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use bytes::Bytes;
use glam::{IVec3, Vec3};
use iroh::endpoint::{Connection, Endpoint, VarInt, presets};
use iroh::EndpointAddr;
use tokio::sync::{RwLock, mpsc, watch};
use voxel_core::VoxelId;

use crate::protocol::{
    PlayerId, PlayerInfo, ReliableMessage, UnreliableDatagram, decode_datagram, encode_datagram,
    read_reliable_frame, write_reliable_frame,
};
use crate::ticket::{decode_join_code, encode_join_code};

/// Application-Layer Protocol Negotiation identifier for Echoes Below P2P sessions.
pub const ECHOES_ALPN: &[u8] = b"echoes-below/v1";

/// Maximum duration the host waits for `Endpoint::online()` before emitting a join code.
///
/// Waiting briefly lets `iroh` discover its home relay URL for NAT traversal over the
/// internet, while falling back cleanly to direct/LAN addresses when offline.
const HOST_ONLINE_TIMEOUT: Duration = Duration::from_secs(3);

/// Maximum duration to wait for initial `Handshake` / `Welcome` exchange.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Incoming network events drained synchronously by Bevy's `Update` loop.
#[derive(Clone, Debug)]
pub enum NetEvent {
    HostReady {
        join_code: String,
    },
    Connected {
        local_id: PlayerId,
        level_seed: u64,
        players: Vec<PlayerInfo>,
        voxel_edits: Vec<(IVec3, VoxelId)>,
        beacon_pos_m: Vec3,
    },
    PlayerJoined(PlayerInfo),
    PlayerLeft(PlayerId),
    VoxelDelta {
        coord: IVec3,
        material: VoxelId,
    },
    FlareThrown {
        origin_m: Vec3,
        velocity_m: Vec3,
    },
    BeaconMoved {
        pos_m: Vec3,
    },
    PlayerPose {
        player_id: PlayerId,
        tick: u32,
        feet_pos_m: Vec3,
        yaw: f32,
        pitch: f32,
        crouching: bool,
        flashlight_on: bool,
    },
    VoiceFrame {
        speaker_id: PlayerId,
        seq: u16,
        opus_payload: Vec<u8>,
    },
    Error(String),
    Disconnected(String),
}

enum SessionCommand {
    SendReliable(ReliableMessage),
    SendDatagram(UnreliableDatagram),
    Shutdown,
}

type EventQueue = Arc<Mutex<VecDeque<NetEvent>>>;

fn push_event(events: &EventQueue, event: NetEvent) {
    if let Ok(mut queue) = events.lock() {
        queue.push_back(event);
    }
}

/// Thread-safe (`Send + Sync`), non-blocking P2P networking session handle.
///
/// Internally runs a dedicated multi-threaded Tokio runtime on a background OS thread
/// so synchronous game loops (e.g., Bevy ECS) never block on I/O.
pub struct NetSession {
    is_host: bool,
    local_player_id: Arc<Mutex<Option<PlayerId>>>,
    join_code: Arc<Mutex<Option<String>>>,
    cmd_tx: mpsc::UnboundedSender<SessionCommand>,
    events: EventQueue,
    shutdown_tx: watch::Sender<bool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl NetSession {
    /// Starts a new P2P session as the authoritative host (`PlayerId = 0`).
    ///
    /// Once the `iroh` endpoint is bound (and either online on a relay or timed out
    /// onto local addresses), a [`NetEvent::HostReady`] event is emitted containing
    /// the pasteable join code.
    pub fn start_host(
        player_name: String,
        visor_rgb: [f32; 3],
        level_seed: u64,
        beacon_pos_m: Vec3,
        initial_edits: Vec<(IVec3, VoxelId)>,
    ) -> Result<Self, String> {
        let local_player_id = Arc::new(Mutex::new(Some(0)));
        let join_code = Arc::new(Mutex::new(None));
        let events: EventQueue = Arc::new(Mutex::new(VecDeque::new()));
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let join_code_bg = Arc::clone(&join_code);
        let events_bg = Arc::clone(&events);

        let thread = std::thread::Builder::new()
            .name("voxel-net-host".to_string())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        push_event(
                            &events_bg,
                            NetEvent::Error(format!("failed to create tokio runtime: {e}")),
                        );
                        return;
                    }
                };

                rt.block_on(run_host_task(
                    player_name,
                    visor_rgb,
                    level_seed,
                    beacon_pos_m,
                    initial_edits,
                    join_code_bg,
                    events_bg,
                    cmd_rx,
                    shutdown_rx,
                ));

                rt.shutdown_timeout(Duration::from_millis(500));
            })
            .map_err(|e| format!("failed to spawn host networking thread: {e}"))?;

        Ok(Self {
            is_host: true,
            local_player_id,
            join_code,
            cmd_tx,
            events,
            shutdown_tx,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Connects to an existing host using its pasteable `"ECHO-..."` join code.
    ///
    /// Validates and decodes the join code synchronously before spawning the
    /// background networking thread.
    pub fn start_client(
        join_code: String,
        player_name: String,
        visor_rgb: [f32; 3],
    ) -> Result<Self, String> {
        let host_addr = decode_join_code(&join_code)?;
        let normalized_code = encode_join_code(&host_addr).unwrap_or(join_code);

        let local_player_id = Arc::new(Mutex::new(None));
        let join_code_slot = Arc::new(Mutex::new(Some(normalized_code)));
        let events: EventQueue = Arc::new(Mutex::new(VecDeque::new()));
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let local_id_bg = Arc::clone(&local_player_id);
        let events_bg = Arc::clone(&events);

        let thread = std::thread::Builder::new()
            .name("voxel-net-client".to_string())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        push_event(
                            &events_bg,
                            NetEvent::Error(format!("failed to create tokio runtime: {e}")),
                        );
                        return;
                    }
                };

                rt.block_on(run_client_task(
                    host_addr,
                    player_name,
                    visor_rgb,
                    local_id_bg,
                    events_bg,
                    cmd_rx,
                    shutdown_rx,
                ));

                rt.shutdown_timeout(Duration::from_millis(500));
            })
            .map_err(|e| format!("failed to spawn client networking thread: {e}"))?;

        Ok(Self {
            is_host: false,
            local_player_id,
            join_code: join_code_slot,
            cmd_tx,
            events,
            shutdown_tx,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Returns `true` if this session is the host.
    pub fn is_host(&self) -> bool {
        self.is_host
    }

    /// Returns the local player's assigned [`PlayerId`].
    ///
    /// Immediately `Some(0)` on the host; becomes `Some(id)` on a client after
    /// the host responds with [`ReliableMessage::Welcome`].
    pub fn local_player_id(&self) -> Option<PlayerId> {
        self.local_player_id.lock().ok().and_then(|id| *id)
    }

    /// Returns the pasteable join code for this session once available.
    pub fn join_code(&self) -> Option<String> {
        self.join_code.lock().ok().and_then(|c| c.clone())
    }

    /// Sends a reliable message.
    ///
    /// - On the host: updates authoritative tracked state (`VoxelDelta`, `BeaconMoved`,
    ///   `StartDescent`) and broadcasts the message to all connected guests.
    /// - On a client: sends the message to the host, which processes and relays it to
    ///   all other connected guests.
    pub fn send_reliable(&self, msg: ReliableMessage) {
        let _ = self.cmd_tx.send(SessionCommand::SendReliable(msg));
    }

    /// Sends an unreliable, low-latency datagram.
    ///
    /// - On the host: broadcasts to all connected guests.
    /// - On a client: sends to the host, which relays it to all other connected guests.
    pub fn send_datagram(&self, dgram: UnreliableDatagram) {
        let _ = self.cmd_tx.send(SessionCommand::SendDatagram(dgram));
    }

    /// Non-blocking drain of all queued [`NetEvent`]s since the last poll.
    pub fn poll_events(&self) -> Vec<NetEvent> {
        match self.events.lock() {
            Ok(mut queue) => queue.drain(..).collect(),
            Err(_) => Vec::new(),
        }
    }
}

impl Drop for NetSession {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        let _ = self.cmd_tx.send(SessionCommand::Shutdown);
        if let Ok(mut guard) = self.thread.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
        }
    }
}

struct PeerHandle {
    reliable_tx: mpsc::UnboundedSender<ReliableMessage>,
    conn: Connection,
}

struct HostState {
    level_seed: u64,
    beacon_pos_m: Vec3,
    voxel_edits: HashMap<IVec3, VoxelId>,
    next_player_id: PlayerId,
    players: HashMap<PlayerId, PlayerInfo>,
    peers: HashMap<PlayerId, PeerHandle>,
}

impl HostState {
    fn sorted_players(&self) -> Vec<PlayerInfo> {
        let mut list: Vec<PlayerInfo> = self.players.values().cloned().collect();
        list.sort_by_key(|p| p.id);
        list
    }

    fn voxel_edits_vec(&self) -> Vec<(IVec3, VoxelId)> {
        self.voxel_edits.iter().map(|(&k, &v)| (k, v)).collect()
    }

    fn mutates_state(msg: &ReliableMessage) -> bool {
        matches!(
            msg,
            ReliableMessage::VoxelDelta { .. }
                | ReliableMessage::BeaconMoved { .. }
                | ReliableMessage::StartDescent { .. }
        )
    }

    fn apply_state_mutation(&mut self, msg: &ReliableMessage) {
        match msg {
            ReliableMessage::VoxelDelta { coord, material } => {
                self.voxel_edits.insert(*coord, *material);
            }
            ReliableMessage::BeaconMoved { pos_m } => {
                self.beacon_pos_m = *pos_m;
            }
            ReliableMessage::StartDescent { level_seed, .. } => {
                self.level_seed = *level_seed;
                self.voxel_edits.clear();
            }
            _ => {}
        }
    }

    fn broadcast_reliable(&self, msg: &ReliableMessage, exclude: Option<PlayerId>) {
        for (&peer_id, peer) in &self.peers {
            if Some(peer_id) == exclude {
                continue;
            }
            let _ = peer.reliable_tx.send(msg.clone());
        }
    }

    fn broadcast_datagram_bytes(&self, payload: &Bytes, exclude: Option<PlayerId>) {
        for (&peer_id, peer) in &self.peers {
            if Some(peer_id) == exclude {
                continue;
            }
            let _ = peer.conn.send_datagram(payload.clone());
        }
    }
}

/// Ensures the host [`EndpointAddr`] always includes at least one directly dialable
/// local socket address (e.g., loopback in air-gapped or test environments) in addition
/// to any discovered LAN addresses and relay URLs.
fn build_host_endpoint_addr(endpoint: &Endpoint) -> EndpointAddr {
    let mut addr = endpoint.addr();
    if addr.ip_addrs().next().is_none() {
        for bound in endpoint.bound_sockets() {
            let dialable = match bound.ip() {
                IpAddr::V4(ip) if ip.is_unspecified() => {
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bound.port())
                }
                IpAddr::V6(ip) if ip.is_unspecified() => {
                    SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), bound.port())
                }
                _ => bound,
            };
            addr = addr.with_ip_addr(dialable);
        }
    }
    addr
}

#[allow(clippy::too_many_arguments)]
async fn run_host_task(
    player_name: String,
    visor_rgb: [f32; 3],
    level_seed: u64,
    beacon_pos_m: Vec3,
    initial_edits: Vec<(IVec3, VoxelId)>,
    join_code_slot: Arc<Mutex<Option<String>>>,
    events: EventQueue,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let endpoint = match Endpoint::builder(presets::N0)
        .alpns(vec![ECHOES_ALPN.to_vec()])
        .bind()
        .await
    {
        Ok(ep) => ep,
        Err(e) => {
            push_event(
                &events,
                NetEvent::Error(format!("failed to bind host iroh endpoint: {e}")),
            );
            return;
        }
    };

    // Wait briefly for relay connectivity so remote peers can traverse NAT,
    // while still becoming ready offline on LAN / loopback.
    tokio::select! {
        _ = shutdown_rx.changed() => {
            let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
            return;
        }
        _ = tokio::time::timeout(HOST_ONLINE_TIMEOUT, endpoint.online()) => {}
    }

    let addr = build_host_endpoint_addr(&endpoint);
    let code = match encode_join_code(&addr) {
        Ok(code) => code,
        Err(e) => {
            push_event(&events, NetEvent::Error(e));
            let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
            return;
        }
    };

    if let Ok(mut slot) = join_code_slot.lock() {
        *slot = Some(code.clone());
    }
    push_event(&events, NetEvent::HostReady { join_code: code });

    let mut voxel_edits = HashMap::with_capacity(initial_edits.len());
    for (coord, mat) in initial_edits {
        voxel_edits.insert(coord, mat);
    }

    let mut players = HashMap::new();
    players.insert(
        0,
        PlayerInfo {
            id: 0,
            name: player_name,
            visor_rgb,
        },
    );

    let state = Arc::new(RwLock::new(HostState {
        level_seed,
        beacon_pos_m,
        voxel_edits,
        next_player_id: 1,
        players,
        peers: HashMap::new(),
    }));

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                break;
            }
            maybe_cmd = cmd_rx.recv() => {
                match maybe_cmd {
                    Some(SessionCommand::SendReliable(msg)) => {
                        if HostState::mutates_state(&msg) {
                            let mut guard = state.write().await;
                            guard.apply_state_mutation(&msg);
                            guard.broadcast_reliable(&msg, None);
                        } else {
                            let guard = state.read().await;
                            guard.broadcast_reliable(&msg, None);
                        }
                    }
                    Some(SessionCommand::SendDatagram(dgram)) => {
                        match encode_datagram(&dgram) {
                            Ok(raw) => {
                                let bytes = Bytes::from(raw);
                                let guard = state.read().await;
                                guard.broadcast_datagram_bytes(&bytes, None);
                            }
                            Err(e) => {
                                push_event(&events, NetEvent::Error(format!("encode datagram failed: {e}")));
                            }
                        }
                    }
                    Some(SessionCommand::Shutdown) | None => {
                        break;
                    }
                }
            }
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else {
                    break;
                };
                let state_clone = Arc::clone(&state);
                let events_clone = Arc::clone(&events);
                let peer_shutdown = shutdown_rx.clone();
                tokio::spawn(async move {
                    handle_incoming_guest(incoming, state_clone, events_clone, peer_shutdown).await;
                });
            }
        }
    }

    // Close all peer connections and the endpoint cleanly.
    {
        let mut guard = state.write().await;
        for (_, peer) in guard.peers.drain() {
            peer.conn.close(VarInt::from_u32(0), b"host shutdown");
        }
    }
    let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
}

async fn handle_incoming_guest(
    incoming: iroh::endpoint::Incoming,
    state: Arc<RwLock<HostState>>,
    events: EventQueue,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let conn = match incoming.await {
        Ok(c) => c,
        Err(e) => {
            push_event(
                &events,
                NetEvent::Error(format!("failed to accept peer connection: {e}")),
            );
            return;
        }
    };

    let (mut send_stream, mut recv_stream) =
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, conn.accept_bi()).await {
            Ok(Ok(streams)) => streams,
            Ok(Err(e)) => {
                push_event(
                    &events,
                    NetEvent::Error(format!("failed to accept peer reliable stream: {e}")),
                );
                conn.close(VarInt::from_u32(1), b"stream accept failed");
                return;
            }
            Err(_) => {
                conn.close(VarInt::from_u32(1), b"handshake timeout");
                return;
            }
        };

    let first_msg =
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, read_reliable_frame(&mut recv_stream)).await {
            Ok(Ok(Some(msg))) => msg,
            Ok(Ok(None)) => {
                conn.close(VarInt::from_u32(1), b"closed before handshake");
                return;
            }
            Ok(Err(e)) => {
                push_event(
                    &events,
                    NetEvent::Error(format!("peer handshake read error: {e}")),
                );
                conn.close(VarInt::from_u32(1), b"handshake read error");
                return;
            }
            Err(_) => {
                conn.close(VarInt::from_u32(1), b"handshake timeout");
                return;
            }
        };

    let (player_name, visor_rgb) = match first_msg {
        ReliableMessage::Handshake {
            player_name,
            visor_rgb,
        } => (player_name, visor_rgb),
        _ => {
            conn.close(VarInt::from_u32(1), b"expected handshake");
            return;
        }
    };

    let (reliable_tx, mut reliable_rx) = mpsc::unbounded_channel::<ReliableMessage>();

    let (player_id, player_info, welcome_msg) = {
        let mut guard = state.write().await;
        let player_id = guard.next_player_id;
        guard.next_player_id = guard.next_player_id.saturating_add(1);

        let info = PlayerInfo {
            id: player_id,
            name: player_name,
            visor_rgb,
        };
        guard.players.insert(player_id, info.clone());
        guard.peers.insert(
            player_id,
            PeerHandle {
                reliable_tx,
                conn: conn.clone(),
            },
        );

        let welcome = ReliableMessage::Welcome {
            assigned_id: player_id,
            level_seed: guard.level_seed,
            players: guard.sorted_players(),
            voxel_edits: guard.voxel_edits_vec(),
            beacon_pos_m: guard.beacon_pos_m,
        };

        // Notify existing guests about the new player.
        guard.broadcast_reliable(&ReliableMessage::PlayerJoined(info.clone()), Some(player_id));

        (player_id, info, welcome)
    };

    // Deliver Welcome strictly before any relayed messages.
    if let Err(e) = write_reliable_frame(&mut send_stream, &welcome_msg).await {
        push_event(
            &events,
            NetEvent::Error(format!("failed to send welcome to player {player_id}: {e}")),
        );
        cleanup_peer(player_id, &state, &events, &conn).await;
        return;
    }

    push_event(&events, NetEvent::PlayerJoined(player_info));

    // Spawn dedicated reliable writer task (cancel-safe framing).
    let writer_handle = tokio::spawn(async move {
        while let Some(msg) = reliable_rx.recv().await {
            if write_reliable_frame(&mut send_stream, &msg).await.is_err() {
                break;
            }
        }
        let _ = send_stream.finish();
    });

    // Spawn dedicated reliable reader task (cancel-safe framing).
    let state_rel = Arc::clone(&state);
    let events_rel = Arc::clone(&events);
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<()>();
    let done_tx_rel = done_tx.clone();
    let reader_handle = tokio::spawn(async move {
        while let Ok(Some(msg)) = read_reliable_frame(&mut recv_stream).await {
            if HostState::mutates_state(&msg) {
                let mut guard = state_rel.write().await;
                guard.apply_state_mutation(&msg);
                guard.broadcast_reliable(&msg, Some(player_id));
            } else {
                let guard = state_rel.read().await;
                guard.broadcast_reliable(&msg, Some(player_id));
            }

            emit_reliable_event(&events_rel, msg);
        }
        let _ = done_tx_rel.send(());
    });

    // Spawn dedicated datagram reader task.
    let conn_dgram = conn.clone();
    let state_dgram = Arc::clone(&state);
    let events_dgram = Arc::clone(&events);
    let done_tx_dgram = done_tx;
    let dgram_handle = tokio::spawn(async move {
        while let Ok(raw) = conn_dgram.read_datagram().await {
            match decode_datagram(&raw) {
                Ok(dgram) => {
                    {
                        let guard = state_dgram.read().await;
                        guard.broadcast_datagram_bytes(&raw, Some(player_id));
                    }
                    emit_datagram_event(&events_dgram, dgram);
                }
                Err(e) => {
                    push_event(
                        &events_dgram,
                        NetEvent::Error(format!(
                            "invalid datagram from player {player_id}: {e}"
                        )),
                    );
                }
            }
        }
        let _ = done_tx_dgram.send(());
    });

    tokio::select! {
        _ = done_rx.recv() => {}
        _ = conn.closed() => {}
        _ = shutdown_rx.changed() => {}
    }

    writer_handle.abort();
    reader_handle.abort();
    dgram_handle.abort();

    cleanup_peer(player_id, &state, &events, &conn).await;
}

async fn cleanup_peer(
    player_id: PlayerId,
    state: &Arc<RwLock<HostState>>,
    events: &EventQueue,
    conn: &Connection,
) {
    conn.close(VarInt::from_u32(0), b"peer disconnected");

    let removed = {
        let mut guard = state.write().await;
        guard.peers.remove(&player_id);
        let existed = guard.players.remove(&player_id).is_some();
        if existed {
            guard.broadcast_reliable(&ReliableMessage::PlayerLeft { player_id }, None);
        }
        existed
    };

    if removed {
        push_event(events, NetEvent::PlayerLeft(player_id));
    }
}

async fn run_client_task(
    host_addr: EndpointAddr,
    player_name: String,
    visor_rgb: [f32; 3],
    local_player_id: Arc<Mutex<Option<PlayerId>>>,
    events: EventQueue,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let endpoint = match Endpoint::builder(presets::N0).bind().await {
        Ok(ep) => ep,
        Err(e) => {
            push_event(
                &events,
                NetEvent::Error(format!("failed to bind client iroh endpoint: {e}")),
            );
            push_event(
                &events,
                NetEvent::Disconnected(format!("endpoint bind failed: {e}")),
            );
            return;
        }
    };

    let conn = tokio::select! {
        _ = shutdown_rx.changed() => {
            let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
            return;
        }
        res = endpoint.connect(host_addr, ECHOES_ALPN) => {
            match res {
                Ok(c) => c,
                Err(e) => {
                    let msg = format!("failed to connect to host: {e}");
                    push_event(&events, NetEvent::Error(msg.clone()));
                    push_event(&events, NetEvent::Disconnected(msg));
                    let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
                    return;
                }
            }
        }
    };

    let (mut send_stream, mut recv_stream) = match conn.open_bi().await {
        Ok(streams) => streams,
        Err(e) => {
            let msg = format!("failed to open reliable stream to host: {e}");
            push_event(&events, NetEvent::Error(msg.clone()));
            push_event(&events, NetEvent::Disconnected(msg));
            conn.close(VarInt::from_u32(1), b"open_bi failed");
            let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
            return;
        }
    };

    let handshake = ReliableMessage::Handshake {
        player_name,
        visor_rgb,
    };
    if let Err(e) = write_reliable_frame(&mut send_stream, &handshake).await {
        let msg = format!("failed to send handshake: {e}");
        push_event(&events, NetEvent::Error(msg.clone()));
        push_event(&events, NetEvent::Disconnected(msg));
        conn.close(VarInt::from_u32(1), b"handshake write failed");
        let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
        return;
    }

    let welcome = tokio::select! {
        _ = shutdown_rx.changed() => {
            conn.close(VarInt::from_u32(0), b"client shutdown");
            let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
            return;
        }
        res = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_reliable_frame(&mut recv_stream)) => {
            match res {
                Ok(Ok(Some(msg))) => msg,
                Ok(Ok(None)) => {
                    push_event(&events, NetEvent::Disconnected("host closed connection during handshake".to_string()));
                    conn.close(VarInt::from_u32(1), b"closed during handshake");
                    let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
                    return;
                }
                Ok(Err(e)) => {
                    let msg = format!("failed reading host welcome: {e}");
                    push_event(&events, NetEvent::Error(msg.clone()));
                    push_event(&events, NetEvent::Disconnected(msg));
                    conn.close(VarInt::from_u32(1), b"welcome read failed");
                    let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
                    return;
                }
                Err(_) => {
                    let msg = "timed out waiting for host welcome".to_string();
                    push_event(&events, NetEvent::Error(msg.clone()));
                    push_event(&events, NetEvent::Disconnected(msg));
                    conn.close(VarInt::from_u32(1), b"welcome timeout");
                    let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
                    return;
                }
            }
        }
    };

    match welcome {
        ReliableMessage::Welcome {
            assigned_id,
            level_seed,
            players,
            voxel_edits,
            beacon_pos_m,
        } => {
            if let Ok(mut id_slot) = local_player_id.lock() {
                *id_slot = Some(assigned_id);
            }
            push_event(
                &events,
                NetEvent::Connected {
                    local_id: assigned_id,
                    level_seed,
                    players,
                    voxel_edits,
                    beacon_pos_m,
                },
            );
        }
        other => {
            let msg = format!("unexpected initial message from host: {other:?}");
            push_event(&events, NetEvent::Error(msg.clone()));
            push_event(&events, NetEvent::Disconnected(msg));
            conn.close(VarInt::from_u32(1), b"expected welcome");
            let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
            return;
        }
    }

    // Spawn dedicated reliable writer task (cancel-safe).
    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<ReliableMessage>();
    let (disc_tx, mut disc_rx) = mpsc::unbounded_channel::<String>();

    let disc_tx_writer = disc_tx.clone();
    let writer_handle = tokio::spawn(async move {
        while let Some(msg) = write_rx.recv().await {
            if let Err(e) = write_reliable_frame(&mut send_stream, &msg).await {
                let _ = disc_tx_writer.send(format!("reliable send failed: {e}"));
                break;
            }
        }
        let _ = send_stream.finish();
    });

    // Spawn dedicated reliable reader task (cancel-safe).
    let events_rel = Arc::clone(&events);
    let disc_tx_rel = disc_tx.clone();
    let reader_handle = tokio::spawn(async move {
        loop {
            match read_reliable_frame(&mut recv_stream).await {
                Ok(Some(msg)) => emit_reliable_event(&events_rel, msg),
                Ok(None) => {
                    let _ = disc_tx_rel.send("host closed reliable stream".to_string());
                    break;
                }
                Err(e) => {
                    let _ = disc_tx_rel.send(format!("reliable stream error: {e}"));
                    break;
                }
            }
        }
    });

    // Spawn dedicated datagram reader task.
    let conn_dgram = conn.clone();
    let events_dgram = Arc::clone(&events);
    let disc_tx_dgram = disc_tx;
    let dgram_handle = tokio::spawn(async move {
        loop {
            match conn_dgram.read_datagram().await {
                Ok(raw) => match decode_datagram(&raw) {
                    Ok(dgram) => emit_datagram_event(&events_dgram, dgram),
                    Err(e) => {
                        push_event(
                            &events_dgram,
                            NetEvent::Error(format!("invalid datagram from host: {e}")),
                        );
                    }
                },
                Err(e) => {
                    let _ = disc_tx_dgram.send(format!("datagram receive closed: {e}"));
                    break;
                }
            }
        }
    });

    let mut local_shutdown = false;
    let mut disconnect_reason: Option<String> = None;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                local_shutdown = true;
                break;
            }
            reason = disc_rx.recv() => {
                disconnect_reason = Some(reason.unwrap_or_else(|| "connection closed".to_string()));
                break;
            }
            maybe_cmd = cmd_rx.recv() => {
                match maybe_cmd {
                    Some(SessionCommand::SendReliable(msg)) => {
                        let _ = write_tx.send(msg);
                    }
                    Some(SessionCommand::SendDatagram(dgram)) => {
                        match encode_datagram(&dgram) {
                            Ok(raw) => {
                                let _ = conn.send_datagram(Bytes::from(raw));
                            }
                            Err(e) => {
                                push_event(&events, NetEvent::Error(format!("encode datagram failed: {e}")));
                            }
                        }
                    }
                    Some(SessionCommand::Shutdown) | None => {
                        local_shutdown = true;
                        break;
                    }
                }
            }
        }
    }

    writer_handle.abort();
    reader_handle.abort();
    dgram_handle.abort();

    if !local_shutdown {
        push_event(
            &events,
            NetEvent::Disconnected(
                disconnect_reason.unwrap_or_else(|| "disconnected from host".to_string()),
            ),
        );
    }

    conn.close(VarInt::from_u32(0), b"client disconnect");
    let _ = tokio::time::timeout(Duration::from_millis(500), endpoint.close()).await;
}

fn emit_reliable_event(events: &EventQueue, msg: ReliableMessage) {
    match msg {
        ReliableMessage::PlayerJoined(info) => push_event(events, NetEvent::PlayerJoined(info)),
        ReliableMessage::PlayerLeft { player_id } => {
            push_event(events, NetEvent::PlayerLeft(player_id))
        }
        ReliableMessage::VoxelDelta { coord, material } => {
            push_event(events, NetEvent::VoxelDelta { coord, material })
        }
        ReliableMessage::FlareThrown {
            origin_m,
            velocity_m,
        } => push_event(
            events,
            NetEvent::FlareThrown {
                origin_m,
                velocity_m,
            },
        ),
        ReliableMessage::BeaconMoved { pos_m } => {
            push_event(events, NetEvent::BeaconMoved { pos_m })
        }
        ReliableMessage::Handshake { .. }
        | ReliableMessage::Welcome { .. }
        | ReliableMessage::StartDescent { .. } => {}
    }
}

fn emit_datagram_event(events: &EventQueue, dgram: UnreliableDatagram) {
    match dgram {
        UnreliableDatagram::PlayerPose {
            player_id,
            tick,
            feet_pos_m,
            yaw,
            pitch,
            crouching,
            flashlight_on,
        } => push_event(
            events,
            NetEvent::PlayerPose {
                player_id,
                tick,
                feet_pos_m,
                yaw,
                pitch,
                crouching,
                flashlight_on,
            },
        ),
        UnreliableDatagram::VoiceFrame {
            speaker_id,
            seq,
            opus_payload,
        } => push_event(
            events,
            NetEvent::VoiceFrame {
                speaker_id,
                seq,
                opus_payload,
            },
        ),
    }
}
