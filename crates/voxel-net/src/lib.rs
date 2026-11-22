//! Zero-API P2P co-op networking layer built on `iroh` QUIC.
//!
//! Hosts receive a pasteable `"ECHO-..."` join code encoding their `iroh::EndpointAddr`
//! (both direct addresses and relay URL when online), allowing friends to connect over
//! the internet without port forwarding or dedicated matchmaking servers.

pub mod protocol;
pub mod session;
pub mod ticket;

pub use protocol::{
    ElevatorPhase, HeldItemNetKind, MonsterBehaviorState, PlayerHealthStatus, PlayerId, PlayerInfo,
    ReliableMessage, UnreliableDatagram, WorldItemKind, WorldItemNetState, decode_datagram,
    decode_reliable, encode_datagram, encode_reliable,
};
pub use session::{NetEvent, NetSession};
pub use ticket::{decode_join_code, encode_join_code};

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::time::{Duration, Instant};

    use glam::{IVec3, Vec3};
    use iroh::{EndpointAddr, SecretKey};
    use voxel_core::VoxelId;

    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn net_session_is_send_and_sync() {
        assert_send_sync::<NetSession>();
    }

    #[test]
    fn reliable_message_roundtrip() {
        let sample_relic = WorldItemNetState {
            id: 7,
            kind: WorldItemKind::HeavyRelic {
                value: 250,
                weight_kg: 48.5,
            },
            pos_m: Vec3::new(10.0, 29.5, -14.2),
            vel_m: Vec3::new(0.0, -0.5, 0.25),
            in_shed: false,
            carried_by: vec![0, 1],
            active_timer_s: None,
        };
        let sample_decoy = WorldItemNetState {
            id: 12,
            kind: WorldItemKind::NoiseDecoy { count: 2 },
            pos_m: Vec3::new(1.0, 2.0, 3.0),
            vel_m: Vec3::ZERO,
            in_shed: true,
            carried_by: vec![],
            active_timer_s: Some(4.5),
        };

        let all_messages = vec![
            ReliableMessage::Handshake {
                player_name: "Scout".to_string(),
                visor_rgb: [0.1, 0.9, 0.5],
            },
            ReliableMessage::Welcome {
                assigned_id: 2,
                level_seed: 0xDEAD_BEEF,
                depth_tier: 2,
                elevator_phase: ElevatorPhase::CaveIdle,
                credits: 420,
                world_items: vec![sample_relic.clone(), sample_decoy.clone()],
                players: vec![PlayerInfo {
                    id: 0,
                    name: "Host".to_string(),
                    visor_rgb: [0.2, 0.8, 1.0],
                }],
                voxel_edits: vec![(IVec3::new(1, 2, 3), VoxelId::AcousticFoam)],
                beacon_pos_m: Vec3::new(5.0, 6.0, 7.0),
            },
            ReliableMessage::StartDescent {
                level_seed: 998877,
                depth_tier: 3,
            },
            ReliableMessage::VoxelDelta {
                coord: IVec3::new(12, 34, 56),
                material: VoxelId::AcousticFoam,
            },
            ReliableMessage::ElevatorStateSync {
                phase: ElevatorPhase::WinchActive,
                platform_y_m: 29.7,
                phase_timer_s: 32.5,
                depth_tier: 2,
                cave_seed: 123456,
            },
            ReliableMessage::RequestElevatorAction { depth_tier: 3 },
            ReliableMessage::WorldItemSpawn(sample_relic.clone()),
            ReliableMessage::WorldItemUpdate(sample_decoy),
            ReliableMessage::WorldItemSpawn(WorldItemNetState {
                id: 13,
                kind: WorldItemKind::BonusLoot { value: 65 },
                pos_m: Vec3::ZERO,
                vel_m: Vec3::ZERO,
                in_shed: false,
                carried_by: vec![],
                active_timer_s: None,
            }),
            ReliableMessage::WorldItemSpawn(WorldItemNetState {
                id: 14,
                kind: WorldItemKind::BatteryCell { charge: 0.72 },
                pos_m: Vec3::ZERO,
                vel_m: Vec3::ZERO,
                in_shed: true,
                carried_by: vec![],
                active_timer_s: None,
            }),
            ReliableMessage::WorldItemSpawn(WorldItemNetState {
                id: 15,
                kind: WorldItemKind::FlarePack { count: 4 },
                pos_m: Vec3::ZERO,
                vel_m: Vec3::ZERO,
                in_shed: true,
                carried_by: vec![],
                active_timer_s: None,
            }),
            ReliableMessage::WorldItemSpawn(WorldItemNetState {
                id: 16,
                kind: WorldItemKind::FoamCanister { charges: 8 },
                pos_m: Vec3::ZERO,
                vel_m: Vec3::ZERO,
                in_shed: true,
                carried_by: vec![],
                active_timer_s: None,
            }),
            ReliableMessage::WorldItemSpawn(WorldItemNetState {
                id: 17,
                kind: WorldItemKind::AidKit,
                pos_m: Vec3::ZERO,
                vel_m: Vec3::ZERO,
                in_shed: true,
                carried_by: vec![],
                active_timer_s: None,
            }),
            ReliableMessage::WorldItemSpawn(WorldItemNetState {
                id: 18,
                kind: WorldItemKind::Flashlight { battery: 0.95 },
                pos_m: Vec3::ZERO,
                vel_m: Vec3::ZERO,
                in_shed: true,
                carried_by: vec![],
                active_timer_s: None,
            }),
            ReliableMessage::WorldItemRemove { item_id: 7 },
            ReliableMessage::PlayerHealthSync {
                player_id: 1,
                status: PlayerHealthStatus::Healthy,
            },
            ReliableMessage::PlayerHealthSync {
                player_id: 1,
                status: PlayerHealthStatus::Downed {
                    bleedout_s: 45.25,
                    dragged_by: Some(0),
                },
            },
            ReliableMessage::PlayerHealthSync {
                player_id: 1,
                status: PlayerHealthStatus::Dead,
            },
            ReliableMessage::PlayerRevived {
                target_id: 1,
                reviver_id: 0,
            },
            ReliableMessage::CreditsSync {
                credits: 1250,
                unlocked_depth_tier: 3,
            },
            ReliableMessage::PlayerProfileSync {
                player_id: 0,
                player_name: "HostDiver".to_string(),
                visor_rgb: [0.2, 0.8, 1.0],
                inventory_json: r#"{"belt":["flashlight","aid_kit"]}"#.to_string(),
            },
            ReliableMessage::DecoyTriggered {
                pos_m: Vec3::new(33.0, 29.8, -11.5),
            },
            ReliableMessage::MonsterAttack {
                target_player_id: 1,
                monster_pos_m: Vec3::new(34.5, 29.8, -10.0),
            },
        ];

        for msg in all_messages {
            let bytes = encode_reliable(&msg).expect("encode reliable");
            let decoded = decode_reliable(&bytes).expect("decode reliable");
            assert_eq!(decoded, msg);
        }
    }

    #[test]
    fn datagram_roundtrip() {
        let datagrams = vec![
            UnreliableDatagram::PlayerPose {
                player_id: 2,
                tick: 120,
                feet_pos_m: Vec3::new(1.5, 2.0, -3.25),
                yaw: 1.2,
                pitch: -0.35,
                crouching: true,
                flashlight_on: false,
                held_item: HeldItemNetKind::AcousticFoam,
                pickaxe_swing: true,
            },
            UnreliableDatagram::VoiceFrame {
                speaker_id: 1,
                seq: 42,
                opus_payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
            },
            UnreliableDatagram::MonsterPose {
                monster_id: 3,
                pos_m: Vec3::new(42.5, 29.7, -8.25),
                yaw: -2.15,
                state: MonsterBehaviorState::Hunt,
            },
            UnreliableDatagram::ItemPose {
                item_id: 99,
                pos_m: Vec3::new(18.0, 30.1, 4.75),
            },
        ];

        for dgram in datagrams {
            let bytes = encode_datagram(&dgram).expect("encode datagram");
            assert_eq!(decode_datagram(&bytes).expect("decode datagram"), dgram);
        }
    }

    #[test]
    fn join_code_roundtrip() {
        let key = SecretKey::generate();
        let addr = EndpointAddr::new(key.public())
            .with_ip_addr(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 41234))
            .with_relay_url("https://relay.example.com/".parse().expect("relay url"));

        let code = encode_join_code(&addr).expect("encode join code");
        assert!(code.starts_with("ECHO-"), "unexpected prefix: {code}");

        let decoded = decode_join_code(&code).expect("decode join code");
        assert_eq!(decoded, addr);

        // Whitespace and lowercase should also decode cleanly.
        let messy = format!("  {} \n ", code.to_lowercase());
        let decoded_messy = decode_join_code(&messy).expect("decode messy join code");
        assert_eq!(decoded_messy, addr);

        // Invalid prefix should produce a clear error.
        assert!(decode_join_code("BAD-ABCDEF").is_err());
        assert!(decode_join_code("   ").is_err());
    }

    #[test]
    fn loopback_host_client_session_e2e() {
        let seed = 0x5EED_CAFE_1234_5678;
        let initial_beacon = Vec3::new(4.0, 1.5, -2.0);
        let initial_edits = vec![(IVec3::new(3, 4, 5), VoxelId::Steel)];

        let host = NetSession::start_host(
            "HostDiver".to_string(),
            [0.2, 0.8, 1.0],
            seed,
            initial_beacon,
            initial_edits.clone(),
        )
        .expect("start host");

        assert!(host.is_host());
        assert_eq!(host.local_player_id(), Some(0));

        // 1. Wait for HostReady { join_code }.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut join_code = None;
        while Instant::now() < deadline && join_code.is_none() {
            for ev in host.poll_events() {
                match ev {
                    NetEvent::HostReady { join_code: code } => join_code = Some(code),
                    NetEvent::Error(e) => panic!("unexpected host error: {e}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let join_code = join_code.expect("host did not emit HostReady in time");
        assert_eq!(host.join_code().as_deref(), Some(join_code.as_str()));

        // Update host authoritative state before client joins to verify atomic Welcome sync.
        let initial_item = WorldItemNetState {
            id: 1,
            kind: WorldItemKind::BatteryCell { charge: 0.85 },
            pos_m: Vec3::new(2.0, 87.3, 1.0),
            vel_m: Vec3::ZERO,
            in_shed: true,
            carried_by: vec![],
            active_timer_s: None,
        };
        host.send_reliable(ReliableMessage::CreditsSync {
            credits: 350,
            unlocked_depth_tier: 2,
        });
        host.send_reliable(ReliableMessage::WorldItemSpawn(initial_item.clone()));
        host.send_reliable(ReliableMessage::ElevatorStateSync {
            phase: ElevatorPhase::ShedIdle,
            platform_y_m: 87.3,
            phase_timer_s: 0.0,
            depth_tier: 2,
            cave_seed: seed,
        });
        std::thread::sleep(Duration::from_millis(50));

        // 2. Start client with the host's join code.
        let client = NetSession::start_client(
            join_code,
            "GuestScout".to_string(),
            [1.0, 0.4, 0.2],
        )
        .expect("start client");
        assert!(!client.is_host());

        // 3. Verify client receives Connected and host receives PlayerJoined.
        let mut client_connected = false;
        let mut host_saw_guest = false;
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && !(client_connected && host_saw_guest) {
            for ev in client.poll_events() {
                match ev {
                    NetEvent::Connected {
                        local_id,
                        level_seed,
                        depth_tier,
                        elevator_phase,
                        credits,
                        world_items,
                        players,
                        voxel_edits,
                        beacon_pos_m,
                    } => {
                        assert_eq!(local_id, 1);
                        assert_eq!(level_seed, seed);
                        assert_eq!(depth_tier, 2);
                        assert_eq!(elevator_phase, ElevatorPhase::ShedIdle);
                        assert_eq!(credits, 350);
                        assert_eq!(world_items, vec![initial_item.clone()]);
                        assert_eq!(beacon_pos_m, initial_beacon);
                        assert_eq!(voxel_edits, initial_edits);
                        assert_eq!(players.len(), 2);
                        assert_eq!(players[0].id, 0);
                        assert_eq!(players[0].name, "HostDiver");
                        assert_eq!(players[1].id, 1);
                        assert_eq!(players[1].name, "GuestScout");
                        client_connected = true;
                    }
                    NetEvent::Error(e) => panic!("unexpected client error: {e}"),
                    NetEvent::Disconnected(r) => panic!("client disconnected early: {r}"),
                    _ => {}
                }
            }
            for ev in host.poll_events() {
                match ev {
                    NetEvent::PlayerJoined(info) => {
                        assert_eq!(info.id, 1);
                        assert_eq!(info.name, "GuestScout");
                        assert_eq!(info.visor_rgb, [1.0, 0.4, 0.2]);
                        host_saw_guest = true;
                    }
                    NetEvent::Error(e) => panic!("unexpected host error: {e}"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(client_connected, "client did not connect in time");
        assert!(host_saw_guest, "host did not see PlayerJoined in time");
        assert_eq!(client.local_player_id(), Some(1));

        // 4. Send ReliableMessage::VoxelDelta (client -> host) and UnreliableDatagram::VoiceFrame (host -> client & client -> host).
        let delta_coord = IVec3::new(-7, 12, 3);
        client.send_reliable(ReliableMessage::VoxelDelta {
            coord: delta_coord,
            material: VoxelId::Air,
        });

        let voice_payload = vec![0x11, 0x22, 0x33, 0x44, 0x55];
        host.send_datagram(UnreliableDatagram::VoiceFrame {
            speaker_id: 0,
            seq: 9,
            opus_payload: voice_payload.clone(),
        });

        let mut host_got_delta = false;
        let mut client_got_voice = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !(host_got_delta && client_got_voice) {
            for ev in host.poll_events() {
                if let NetEvent::VoxelDelta { coord, material } = ev
                    && coord == delta_coord
                    && material == VoxelId::Air
                {
                    host_got_delta = true;
                }
            }
            for ev in client.poll_events() {
                if let NetEvent::VoiceFrame {
                    speaker_id,
                    seq,
                    opus_payload,
                } = ev
                    && speaker_id == 0
                    && seq == 9
                    && opus_payload == voice_payload
                {
                    client_got_voice = true;
                }
            }
            if !client_got_voice {
                // Re-send datagram occasionally in case an unreliable packet dropped during path setup.
                host.send_datagram(UnreliableDatagram::VoiceFrame {
                    speaker_id: 0,
                    seq: 9,
                    opus_payload: voice_payload.clone(),
                });
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        assert!(host_got_delta, "host did not receive VoxelDelta");
        assert!(client_got_voice, "client did not receive VoiceFrame");

        // 4b. Live visor color & profile sync (Host -> Client and Client -> Host)
        let host_new_visor = [0.98, 0.24, 0.28];
        host.send_reliable(ReliableMessage::PlayerProfileSync {
            player_id: 0,
            player_name: "HostDiver".to_string(),
            visor_rgb: host_new_visor,
            inventory_json: String::new(),
        });
        let guest_new_visor = [0.22, 0.95, 0.42];
        let guest_inv_json = r#"{"belt_capacity":3}"#.to_string();
        client.send_reliable(ReliableMessage::PlayerProfileSync {
            player_id: 99, // Host must overwrite with authenticated guest ID (1)
            player_name: "GuestScout".to_string(),
            visor_rgb: guest_new_visor,
            inventory_json: guest_inv_json.clone(),
        });

        let mut client_saw_host_visor = false;
        let mut host_saw_guest_profile = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !(client_saw_host_visor && host_saw_guest_profile) {
            for ev in client.poll_events() {
                if let NetEvent::PlayerProfileSync {
                    player_id,
                    visor_rgb,
                    ..
                } = ev
                    && player_id == 0
                    && visor_rgb == host_new_visor
                {
                    client_saw_host_visor = true;
                }
            }
            for ev in host.poll_events() {
                if let NetEvent::PlayerProfileSync {
                    player_id,
                    visor_rgb,
                    inventory_json,
                    ..
                } = ev
                    && player_id == 1
                    && visor_rgb == guest_new_visor
                    && inventory_json == guest_inv_json
                {
                    host_saw_guest_profile = true;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(client_saw_host_visor, "client did not receive host visor color update");
        assert!(host_saw_guest_profile, "host did not receive guest visor & inventory update");

        // 5. Verify clean drop/shutdown and PlayerLeft notification on host.
        drop(client);
        let mut host_saw_left = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !host_saw_left {
            for ev in host.poll_events() {
                if let NetEvent::PlayerLeft(1) = ev {
                    host_saw_left = true;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(host_saw_left, "host did not receive PlayerLeft(1) after client dropped");

        drop(host);
    }
}
