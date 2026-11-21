# AGENTS.md — Echoes Below (Rust Co-op Voxel Acoustic Game)

This file is automatically loaded by Antigravity (`agy`) instances working in this repository (Windows, macOS, Linux). Read [DESIGN.md](./DESIGN.md) for the full game direction & technical blueprint, and [HANDOFF.md](./HANDOFF.md) for Windows setup and current Week 3+ continuation tasks.

---

## Core Project Identity & Hard Constraints

- **Game:** *Echoes Below* — co-op dark voxel cave-extraction game for small friend groups (2–6 players).
- **Signature Pillar:** Physically grounded **voxel-density muffling** (3-band dB/m DDA occlusion) + **3D path-traced reflections** (directional early echo taps + dynamic 8-ch Hadamard FDN cavern reverb) processing **both game SFX and built-in proximity voice chat**, and driving **blind acoustic enemies** through the exact same physics.
- **Zero-API Co-op:** Serverless P2P via `iroh` (QUIC + NAT hole-punching + stateless relay fallback). Host generates a copy-pasteable `ECHO-...` join code; guests paste to join. Never introduce external matchmaking APIs or accounts.
- **Scale-Agnostic Voxels (`1/2` now → `1/4` viable later):** All physics, player dimensions, speed of sound (`343 m/s`), and material attenuation (`dB/m`) MUST be expressed in **SI meters** via `VoxelConfig::voxel_size_m` (`0.90 m` default, cleanly switchable to `0.45 m`). Never hardcode `1 voxel = 1 meter`.
- **Auto-step rule:** Only auto-step ledges `<= 0.50 m` (`MAX_AUTO_STEP_M`). At `0.90 m` voxels, players must press `Space` to jump onto blocks.
- **Cross-Platform:** Must compile and run cleanly on **Windows**, **macOS (Apple Silicon)**, and **Linux** (pure Rust `rusty-opus`, `zstd_rust`, `iroh`, `cpal`, Bevy `wgpu`).

---

## Crate Architecture (Strict Decoupling)

Keep all core engine crates **100% free of Bevy dependencies** so they compile fast and stay unit-testable headlessly:

| Crate | Owns |
| :--- | :--- |
| `crates/voxel-core` | `VoxelConfig`, `MaterialDef` (3-band acoustics + destructibility), `32³` chunks with `u64` `8³` brick masks + incremental emissive tracking, flat 3D `Vec<Option<Chunk>>` world, brick/chunk-skipping DDA, greedy mesher with baked AO, deterministic worldgen |
| `crates/voxel-acoustics` | 3-band occlusion, stochastic multi-bounce path-tracer (early taps + RT60), `solve_single_source`, off-thread `AcousticWorker` reading `Arc<RwLock<VoxelWorld>>`, unified `evaluate_ai_hearing` |
| `crates/voxel-audio-dsp` | Lock-free `cpal` output & mic input, zero per-sample trig/transcendentals in 48 kHz callbacks (block-rate FDN + precomputed stereo/tap pans + `rms_sq` mic gate), `rusty-opus` 20 ms codec, PLC decoder pool, WAV loader with exe-relative lookup |
| `crates/voxel-net` | `iroh 1.5` host-authoritative star P2P (`RwLock<HostState>`), `ECHO-...` join tickets, reliable `postcard` streams + unreliable QUIC datagrams (poses & VoIP) |
| `crates/game` | Bevy `0.20` shell (rendering, dynamic lights, UI, physics, tools, gameplay loop, bridges) |

---

## Working Style, Parallel Subagents & Git Discipline

1. **Use parallel subagents aggressively for independent work:**
   - Split multi-crate or disjoint-module features across parallel `self` subagents (`Workspace: "inherit"` when touching completely disjoint files/crates; `Workspace: "branch"` if files overlap).
   - Always settle shared `Cargo.toml` dependency edits *before* spawning subagents, and explicitly forbid subagents from editing files outside their assigned scope.
2. **One clean commit per agent / logical unit of work:**
   - The user prefers each subagent's completed work to be committed as its **own separate commit** (e.g., `feat(voxel-core): ...`, `perf(voxel-audio-dsp): ...`, `feat(game): ...`), then pushed to `origin/main` once verified.
   - Always verify with `cargo test --workspace` and `cargo clippy --workspace --no-deps --all-targets -- -D warnings` (or per-crate clippy) before committing/pushing.
3. **Be candid & harsh on game design tradeoffs:**
   - The user actively wants honest, constructive pushback on gameplay flaws, scope creep, and engine tradeoffs.

---

## Critical Bevy 0.20 & Engine Gotchas Already Solved

Save time—do not re-hit these:

- **Bevy 0.20 API renames:**
  - `Mesh3d(...)` + `MeshMaterial3d(...)` (no `PbrBundle`).
  - `shadow_maps_enabled` (not `shadows_enabled`) on `PointLight` and `SpotLight`.
  - `GlobalAmbientLight` is the world resource; `AmbientLight` is a per-camera override component.
  - `CursorOptions` is queried on `With<PrimaryWindow>` (not mutated on `Window`).
  - `MessageReader<T>` / `MessageWriter<T>` / `#[derive(Message)]` / `app.add_message::<T>()` for buffered streams like `MouseMotion` and custom game messages.
  - `app.insert_non_send(...)` (not `insert_non_send_resource`).
  - `TextFont { font_size: FontSize::Px(14.0), ..default() }`.
  - UI Buttons: use `Node` + Bevy 0.20 entity observers `.observe(|_: On<PointerClick>, ...|)` / `PointerOver` / `PointerOut` (avoid deprecated `Button` / `Interaction`).
  - `GlobalClusterSettings.gpu_clustering.initial_index_list_capacity` is raised to `131_072` in `main.rs` to prevent lighting resize hitches with many flares/crystals.
- **PointLight frustum culling bug:** NEVER attach a `PointLight` directly to an entity that also has a tiny `Mesh3d` (like a flare or beacon sphere)—Bevy will cull the light using the tiny mesh's AABB when the sphere goes off-screen! Always spawn `PointLight` on a **dedicated child entity** with no `Mesh3d`.
- **Fast dev linking vs. standalone releases:**
  - Use `cargo run -p game --features fast-dev` (enables `bevy/dynamic_linking`) for ~1s incremental dev rebuilds.
  - Keep `default = []` so `cargo build --release -p game` and `.github/workflows/release.yml` always produce self-contained static binaries.
- **Real-time audio callback rule:** Zero heap allocations, zero mutex locks, and zero per-sample `sin`/`cos`/`sqrt`/`powf`/`exp`/`log10` inside `cpal` callbacks!
- **Audio asset paths:** `crates/voxel-audio-dsp/src/synth.rs` searches `<exe_dir>/assets/sfx`, `./assets/sfx`, and workspace-relative `assets/sfx`. Any new SFX clips must also be placed under `assets/sfx/` so `.github/workflows/release.yml` bundles them automatically.
