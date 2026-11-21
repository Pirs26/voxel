# HANDOFF.md — Windows Setup & Continuation Guide

Welcome, second Antigravity (`agy`) instance! Everything needed to continue building **Echoes Below** on Windows (or macOS/Linux) is in this repo.

Read **[AGENTS.md](./AGENTS.md)** (auto-loaded) and **[DESIGN.md](./DESIGN.md)** for full architecture, acoustic math, inventory rules, and roadmap.

---

## 1. Windows Setup — What to Download & Install

1. **Git for Windows** — https://git-scm.com/download/win
2. **GitHub CLI (`gh`)** *(recommended for pushing/monitoring releases)* — `winget install GitHub.cli`, then `gh auth login`
3. **Visual Studio Build Tools 2022** (required for Rust's MSVC linker on Windows):
   - Install **Desktop development with C++** workload (includes MSVC compiler + Windows SDK).
4. **Rust Toolchain (`rustup`)** — https://rustup.rs/
   - Installs `rustc`, `cargo`, `rustfmt`, `clippy`. Workspace uses **Rust Edition 2024** (`rustc >= 1.97`, currently verified on `1.100.0`).
5. **No C audio/crypto dependencies needed!**
   - `rusty-opus` is 100% pure Rust (no `libopus`/CMake needed).
   - `zstd_rust` is 100% pure Rust.
   - `iroh` is 100% pure Rust QUIC.
   - *(Optional: `winget install Gyan.FFmpeg` only if you download new `.ogg`/`.mp3` CC0 SFX and want to convert them to 48 kHz mono 16-bit PCM `.wav` in `assets/sfx/`.)*

### Clone & First Run on Windows

```powershell
git clone https://github.com/Pirs26/voxel.git
cd voxel

# Verify all 29 unit/integration tests pass
cargo test --workspace

# Fast local dev run (enables Bevy dynamic_linking for ~1-2s rebuilds!)
cargo run -p game --features fast-dev
```

> **Note:** The first cold build compiles Bevy + Iroh dependencies with `opt-level = 3` (takes a few minutes once), after which incremental rebuilds with `--features fast-dev` are only ~1–3 seconds.

### Creating a New Multi-OS Release (macOS + Windows + Linux)

GitHub Actions ([.github/workflows/release.yml](./.github/workflows/release.yml)) builds native archives for all 3 OSes automatically when a `v*` tag is pushed:

```powershell
git tag -a v0.2.0 -m "v0.2.0 — Week 3 Core Loop"
git push origin v0.2.0
```

Monitor builds at: `https://github.com/Pirs26/voxel/actions`

---

## 2. Current State (Completed Through Week 2 + Perf Pass — Tag `v0.1.0`)

All **29/29 workspace tests pass** with **0 compiler warnings** and **clippy clean**:

- **Week 1 (Done):**
  - Scale-agnostic `voxel-core` (`0.90 m` voxels, flat 3D `Vec<Option<Chunk>>`, `32³` chunks with `u64` `8³` brick masks, true hierarchical chunk/brick DDA skipping verified across 384+ rays, greedy mesher with baked AO & incremental emissives, deterministic Wooden Shed + Steel Mineshaft + Grand Echoing Cavern + Cracked Rock breachable alcove + Bioluminescent Crystals).
  - Real-time `voxel-acoustics` (3-band voxel density muffling + stochastic 3D path-traced early reflection taps + Eyring RT60 cavern reverb + off-thread `AcousticWorker` + `solve_single_source` + `evaluate_ai_hearing`).
  - Real-time lock-free `voxel-audio-dsp` (block-rate 8-ch Hadamard FDN reverb, precomputed stereo/tap panning, zero per-sample trig/transcendentals, CC0 Foley footsteps in `assets/sfx/` with pitch variation).
  - Playable dark voxel cave with draining flashlight (`F`/`R`), bouncing glow-flares (`G`, cached GPU handles + child `PointLight`), breakable Cracked Rock (`LMB`), placeable Acoustic Foam (`RMB`), movable Test Beacon (`E`).
- **Week 2 (Done):**
  - Zero-API serverless P2P co-op in `voxel-net` (`iroh 1.5`, `RwLock<HostState>`, copyable `ECHO-...` join codes, reliable voxel-delta/event streams, unreliable pose + VoIP datagrams).
  - Built-in proximity voice chat (`rusty-opus` 20 ms codec, `cpal` mic capture with `rms_sq` noise gate & whisper-safe AI loudness curve, PLC decoder pool, 6 lock-free live voice slots routed through the exact same voxel muffling + early-echo + FDN reverb chain).
  - `Tab` Co-op & Mic Calibration overlay (Host / Copy Code / Paste & Join / Disconnect / Visor Color / Gate `-`/`+` / Mute `M` / Solo Self-Echo Test `V`), remote blocky player avatars with live flashlight beams, live HUD decibel bar.

---

## 3. Next Up — Week 3 & Week 4 Roadmap (With Parallel Subagent Plan)

Read the full locked gameplay specs in **[DESIGN.md](./DESIGN.md)**—especially the newly locked sections on:
- **Inventory & Equipment:** Start with **2 active belt slots** (upgradeable to 4–5), dedicated **Battery Pouch** (2 → 4–6) with **Subnautica-style `R` + scroll-wheel battery selector** (retaining drained batteries to recharge in the Shed) + **`Q` to drop/toss a physical battery to share with friends**, passive wearable slots (**Headlamp**, **Stealth Boots**, **Pack/Pouch upgrades**), two-handed heavy carry state.
- **Persistent Base Shed & Campaign Saves:** Surface Shed never resets between dives so players can leave gear on shelves/benches/chargers; host saves campaign locally (`saves/<slot>.json`) with shared credits, Shed world items, and per-player inventories keyed by **Player Name** (add Player Name input in lobby/profile!).
- **Core Extraction Loop:**
  1. **Seamless Elevator Descent (Zero Loading Screen):** Lever in Shed → doors close → creaking machinery SFX + flickering cage bulb + shrinking daylight above while underground cave generates/meshes async → doors open into pitch-black cave.
  2. **Heavy Physical Quest Relics:** Weight-scaled carrying (**1 player** = slow loud stone-scraping drag emitting continuous `AcousticEvent`s; **2 players** = clean quiet lift; dropping triggers loud echoing impact) + bonus small loot + throwable **Noise Decoys**.
  3. **Blind Acoustic Stalker AI:** Driven strictly by `voxel_acoustics::evaluate_ai_hearing` reacting to footsteps, voices above whisper curve, dropped items, breached walls, decoys, and the loud elevator winch.
  4. **Downed State & Aid Kit Revive:** Hit puts player Downed (60s bleed-out, slow crawl, muffled whisper still works) → friends can drag them behind Acoustic Foam and revive with a consumable **Aid Kit**.
  5. **Loud Elevator Winch Extraction:** Activating the return winch creates a tense 30–45s loud climax → ascend back to the persistent Shed → sell loot, recharge batteries, buy upgrades/consumables!

### Suggested Parallel Subagent Split for Week 3

Keep each subagent on disjoint modules and create **one clean git commit per subagent**:
1. **Subagent A (`crates/voxel-core` + `crates/voxel-net`):** Split worldgen into persistent Surface Shed + regenerable underground cave per depth tier/seed; add network protocol messages for `ElevatorState`, `WorldItem` (relics, batteries, decoys, loot), `MonsterPose`, `PlayerHealthState`, and `PlayerProfile` sync.
2. **Subagent B (`crates/game/src/inventory.rs` + `save.rs`):** 2→5 belt slots, dedicated Battery Pouch, Subnautica-style `R` + scroll battery swapper + `Q` physical battery/item dropping, passive Headlamp/Boots/Pouch gear, persistent Shed item placement, and host JSON campaign save keyed by Player Name.
3. **Subagent C (`crates/game/src/ai_blind.rs` + `health.rs`):** Blind Acoustic Stalker AI consuming `PlaySpatialSoundEvent::ai_event` and player voice `ai_noise_level` through `voxel_acoustics::evaluate_ai_hearing`, voxel pathing/nav, Downed state, teammate dragging, and consumable Aid Kit revive.
4. **Main Agent (`crates/game/src/elevator.rs`, `relic.rs`, `hud.rs`, integration):** Seamless zero-loading elevator descent + loud extraction winch, weight-scaled 1p-drag / 2p-lift heavy relics, Shed shop/battery charger UI, and full integration + multi-OS test & `v0.2.0` release tag!
