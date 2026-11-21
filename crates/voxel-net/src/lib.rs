//! Zero-API P2P co-op networking layer built on `iroh` QUIC.
//!
//! Hosts receive a pasteable `"ECHO-..."` join code encoding their `iroh::EndpointAddr`
//! (both direct addresses and relay URL when online), allowing friends to connect over
//! the internet without port forwarding or dedicated matchmaking servers.

pub mod protocol;
pub mod session;
pub mod ticket;

pub use protocol::{
    PlayerId, PlayerInfo, ReliableMessage, UnreliableDatagram, decode_datagram, decode_reliable,
    encode_datagram, encode_reliable,
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
        let msg = ReliableMessage::VoxelDelta {
            coord: IVec3::new(12, 34, 56),
            material: VoxelId::AcousticFoam,
        };
        let bytes = encode_reliable(&msg).expect("encode");
        let decoded = decode_reliable(&bytes).expect("decode");
        match decoded {
            ReliableMessage::VoxelDelta { coord, material } => {
                assert_eq!(coord, IVec3::new(12, 34, 56));
                assert_eq!(material, VoxelId::AcousticFoam);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn datagram_roundtrip() {
        let pose = UnreliableDatagram::PlayerPose {
            player_id: 2,
            tick: 120,
            feet_pos_m: Vec3::new(1.5, 2.0, -3.25),
            yaw: 1.2,
            pitch: -0.35,
            crouching: true,
            flashlight_on: false,
        };
        let bytes = encode_datagram(&pose).expect("encode pose");
        assert_eq!(decode_datagram(&bytes).expect("decode pose"), pose);

        let voice = UnreliableDatagram::VoiceFrame {
            speaker_id: 1,
            seq: 42,
            opus_payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let bytes = encode_datagram(&voice).expect("encode voice");
        assert_eq!(decode_datagram(&bytes).expect("decode voice"), voice);
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
                        players,
                        voxel_edits,
                        beacon_pos_m,
                    } => {
                        assert_eq!(local_id, 1);
                        assert_eq!(level_seed, seed);
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
