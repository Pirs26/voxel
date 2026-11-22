use bevy::{
    app::AppExit,
    ecs::system::SystemParam,
    input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
    },
    picking::events::{PointerClick, PointerOut, PointerOver},
    prelude::*,
};
use voxel_acoustics::linear_to_db;

use crate::{
    audio_bridge::{
        AcousticTelemetry, AudioDeviceCatalog, RemoveVoiceSpeakerEvent, VoiceSettings,
    },
    elevator::ElevatorState,
    gameplay::{GameplayToast, SessionLifecycleEvent, ShopActionEvent},
    health::PlayerHealthState,
    inventory::{PlayerInventory, SHOP_CATALOG, ShopCatalogItem},
    net_bridge::{
        CoopMenuOpen, CoopState, VISOR_PALETTE, clear_remote_players, copy_to_clipboard,
        paste_from_clipboard,
    },
    player::PlayerFlashlight,
    relic::WorldItemRegistry,
    save::CampaignState,
    settings::{AppScreen, FOV_MAX_DEG, FOV_MIN_DEG, SettingsMenuOpen, UserSettings},
    shed::{ShedStationState, ShopOverlayOpen},
};

const SLIDER_SEGMENTS: usize = 12;

#[derive(Resource, Default)]
struct NameInputFocused(bool);

// ---- UI Root & Dynamic Text Markers ----

#[derive(Component)]
struct MainMenuRoot;

#[derive(Component)]
struct MainMenuNameBox;

#[derive(Component)]
struct MainMenuNameText;

#[derive(Component)]
struct MainMenuStatusText;

#[derive(Component)]
struct VisorSwatchButton(usize);

#[derive(Component)]
struct SettingsPanelRoot;

#[derive(Component)]
struct OutputDeviceLabel;

#[derive(Component)]
struct InputDeviceLabel;

#[derive(Component)]
struct FullscreenButtonLabel;

#[derive(Component)]
struct SettingsMicStatusText;

#[derive(Component)]
struct InGameHudRoot;

#[derive(Component)]
struct CrosshairNode;

#[derive(Component)]
struct HudText;

#[derive(Component)]
struct PromptText;

#[derive(Component)]
struct ToastText;

#[derive(Component)]
struct BatterySelectorBox;

#[derive(Component)]
struct BatterySelectorText;

#[derive(Component)]
struct BeltHudText;

#[derive(Component)]
struct DamageFlashOverlay;

#[derive(Component)]
struct CoopPanelRoot;

#[derive(Component)]
struct CoopPanelStatusText;

#[derive(Component)]
struct ShopPanelRoot;

#[derive(Component)]
struct ShopHeaderText;

// ---- Interactive Slider & Button Actions ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SliderKind {
    Fov,
    MasterVolume,
    VoiceVolume,
    MicGain,
    MicGate,
}

#[derive(Component, Clone, Copy)]
struct SliderValueText(SliderKind);

#[derive(Component, Clone, Copy)]
struct SliderSegment {
    kind: SliderKind,
    idx: usize,
}

#[derive(Component, Clone, Copy, Debug)]
enum UiAction {
    // Main menu
    PlaySolo,
    HostLobby,
    PasteAndJoin,
    CancelConnecting,
    FocusNameInput,
    SelectVisor(usize),
    OpenSettings,
    CloseSettings,
    QuitGame,

    // Pause / Co-op menu
    CopyJoinCode,
    DisconnectToMenu,
    CycleVisor,
    ToggleMute,
    ToggleSelfEcho,
    ResumeGame,

    // Settings controls
    ToggleFullscreen,
    PrevOutputDevice,
    NextOutputDevice,
    PrevInputDevice,
    NextInputDevice,
    RefreshAudioDevices,
    SliderStep(SliderKind, f32),
    SliderSetFrac(SliderKind, f32),

    // Shed shop
    ShopBuy(ShopCatalogItem),
    ShopTier(u8),
    ShopManualSave,
    ShopClose,
}

#[derive(Message, Clone, Copy, Debug)]
struct TriggerUiAction(UiAction);

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NameInputFocused>()
            .add_message::<TriggerUiAction>()
            .add_systems(Startup, setup_hud)
            .add_systems(
                Update,
                (
                    handle_name_typing_system,
                    handle_ui_actions_system,
                    update_panel_visibility_system,
                    update_menu_hud_texts_system,
                    update_ingame_hud_texts_system,
                    update_settings_widgets_system,
                )
                    .chain(),
            );
    }
}

fn spawn_button(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    action: UiAction,
    base_color: Color,
    hover_color: Color,
) -> Entity {
    let mut ent = Entity::PLACEHOLDER;
    parent
        .spawn((
            Node {
                padding: UiRect::axes(Val::Px(12.0), Val::Px(7.0)),
                margin: UiRect::all(Val::Px(2.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(base_color),
            action,
        ))
        .observe(
            move |_click: On<PointerClick>, mut writer: MessageWriter<TriggerUiAction>| {
                writer.write(TriggerUiAction(action));
            },
        )
        .observe(
            move |over: On<PointerOver>, mut bg_q: Query<&mut BackgroundColor>| {
                if let Ok(mut bg) = bg_q.get_mut(over.entity) {
                    *bg = BackgroundColor(hover_color);
                }
            },
        )
        .observe(
            move |out: On<PointerOut>, mut bg_q: Query<&mut BackgroundColor>| {
                if let Ok(mut bg) = bg_q.get_mut(out.entity) {
                    *bg = BackgroundColor(base_color);
                }
            },
        )
        .with_children(|btn| {
            ent = btn
                .spawn((
                    Text::new(label),
                    TextFont {
                        font_size: FontSize::Px(13.5),
                        ..default()
                    },
                    TextColor(Color::srgb(0.94, 0.97, 0.96)),
                ))
                .id();
        });
    ent
}

fn spawn_std_button(parent: &mut ChildSpawnerCommands, label: &str, action: UiAction) -> Entity {
    spawn_button(
        parent,
        label,
        action,
        Color::srgba(0.15, 0.21, 0.27, 0.96),
        Color::srgba(0.24, 0.38, 0.46, 0.98),
    )
}

fn spawn_accent_button(parent: &mut ChildSpawnerCommands, label: &str, action: UiAction) -> Entity {
    spawn_button(
        parent,
        label,
        action,
        Color::srgba(0.11, 0.36, 0.35, 0.96),
        Color::srgba(0.16, 0.52, 0.50, 0.98),
    )
}

fn spawn_danger_button(parent: &mut ChildSpawnerCommands, label: &str, action: UiAction) -> Entity {
    spawn_button(
        parent,
        label,
        action,
        Color::srgba(0.38, 0.14, 0.15, 0.96),
        Color::srgba(0.55, 0.20, 0.22, 0.98),
    )
}

fn spawn_slider_row(parent: &mut ChildSpawnerCommands, title: &str, kind: SliderKind) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            column_gap: Val::Px(8.0),
            width: Val::Percent(100.0),
            ..default()
        })
        .with_children(|row| {
            row.spawn((
                Text::new(title),
                TextFont {
                    font_size: FontSize::Px(13.5),
                    ..default()
                },
                TextColor(Color::srgb(0.86, 0.91, 0.94)),
                Node {
                    width: Val::Px(150.0),
                    ..default()
                },
            ));

            row.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(3.0),
                ..default()
            })
            .with_children(|ctrl| {
                spawn_std_button(ctrl, "-", UiAction::SliderStep(kind, -1.0));

                for idx in 0..SLIDER_SEGMENTS {
                    let frac = idx as f32 / (SLIDER_SEGMENTS - 1) as f32;
                    ctrl.spawn((
                        Node {
                            width: Val::Px(16.0),
                            height: Val::Px(18.0),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.18, 0.23, 0.28, 0.95)),
                        SliderSegment { kind, idx },
                    ))
                    .observe(
                        move |_click: On<PointerClick>,
                              mut writer: MessageWriter<TriggerUiAction>| {
                            writer.write(TriggerUiAction(UiAction::SliderSetFrac(kind, frac)));
                        },
                    );
                }

                spawn_std_button(ctrl, "+", UiAction::SliderStep(kind, 1.0));
            });

            row.spawn((
                Text::new("--"),
                TextFont {
                    font_size: FontSize::Px(13.5),
                    ..default()
                },
                TextColor(Color::srgb(0.32, 0.94, 0.86)),
                Node {
                    width: Val::Px(72.0),
                    justify_content: JustifyContent::FlexEnd,
                    ..default()
                },
                SliderValueText(kind),
            ));
        });
}

fn setup_hud(mut commands: Commands) {
    // =========================================================================
    // 1. MAIN MENU OVERLAY (Shown on startup & after disconnecting)
    // =========================================================================
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.03, 0.05, 0.72)),
            Visibility::Visible,
            MainMenuRoot,
        ))
        .with_children(|backdrop| {
            backdrop
                .spawn((
                    Node {
                        width: Val::Px(540.0),
                        padding: UiRect::all(Val::Px(24.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(12.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.05, 0.07, 0.11, 0.95)),
                ))
                .with_children(|card| {
                    card.spawn((
                        Text::new("ECHOES BELOW"),
                        TextFont {
                            font_size: FontSize::Px(32.0),
                            ..default()
                        },
                        TextColor(Color::srgb(0.28, 0.94, 0.86)),
                    ));
                    card.spawn((
                        Text::new("Co-op Voxel Acoustic Cave-Extraction  •  Zero-API Iroh P2P"),
                        TextFont {
                            font_size: FontSize::Px(13.5),
                            ..default()
                        },
                        TextColor(Color::srgb(0.68, 0.76, 0.82)),
                    ));

                    // Player Name Input (host saves per-player inventory keyed by this name!)
                    card.spawn(Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(4.0),
                        margin: UiRect::top(Val::Px(6.0)),
                        ..default()
                    })
                    .with_children(|sec| {
                        sec.spawn((
                            Text::new("MINER NAME (Host saves your inventory under this name):"),
                            TextFont {
                                font_size: FontSize::Px(12.0),
                                ..default()
                            },
                            TextColor(Color::srgb(0.75, 0.82, 0.86)),
                        ));
                        sec.spawn((
                            Node {
                                width: Val::Percent(100.0),
                                padding: UiRect::axes(Val::Px(12.0), Val::Px(9.0)),
                                ..default()
                            },
                            BackgroundColor(Color::srgba(0.11, 0.15, 0.21, 0.98)),
                            MainMenuNameBox,
                        ))
                        .observe(
                            |_click: On<PointerClick>,
                             mut writer: MessageWriter<TriggerUiAction>| {
                                writer.write(TriggerUiAction(UiAction::FocusNameInput));
                            },
                        )
                        .with_children(|box_node| {
                            box_node.spawn((
                                Text::new("Miner"),
                                TextFont {
                                    font_size: FontSize::Px(15.0),
                                    ..default()
                                },
                                TextColor(Color::srgb(0.96, 0.98, 1.0)),
                                MainMenuNameText,
                            ));
                        });
                    });

                    // Visor Color Swatch Picker
                    card.spawn(Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(5.0),
                        ..default()
                    })
                    .with_children(|sec| {
                        sec.spawn((
                            Text::new("VISOR COLOR (Visible to teammates on helmet front & back):"),
                            TextFont {
                                font_size: FontSize::Px(12.0),
                                ..default()
                            },
                            TextColor(Color::srgb(0.75, 0.82, 0.86)),
                        ));
                        sec.spawn(Node {
                            flex_direction: FlexDirection::Row,
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: Val::Px(6.0),
                            row_gap: Val::Px(4.0),
                            ..default()
                        })
                        .with_children(|row| {
                            for (idx, (name, rgb)) in VISOR_PALETTE.iter().enumerate() {
                                let swatch_col = Color::srgb(rgb[0], rgb[1], rgb[2]);
                                row.spawn((
                                    Node {
                                        padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                                        flex_direction: FlexDirection::Row,
                                        align_items: AlignItems::Center,
                                        column_gap: Val::Px(6.0),
                                        ..default()
                                    },
                                    BackgroundColor(Color::srgba(0.14, 0.19, 0.25, 0.95)),
                                    VisorSwatchButton(idx),
                                ))
                                .observe(
                                    move |_click: On<PointerClick>,
                                          mut writer: MessageWriter<TriggerUiAction>| {
                                        writer.write(TriggerUiAction(UiAction::SelectVisor(idx)));
                                    },
                                )
                                .with_children(|btn| {
                                    btn.spawn((
                                        Node {
                                            width: Val::Px(12.0),
                                            height: Val::Px(12.0),
                                            ..default()
                                        },
                                        BackgroundColor(swatch_col),
                                    ));
                                    btn.spawn((
                                        Text::new(*name),
                                        TextFont {
                                            font_size: FontSize::Px(12.5),
                                            ..default()
                                        },
                                        TextColor(Color::srgb(0.92, 0.96, 0.98)),
                                    ));
                                });
                            }
                        });
                    });

                    // Primary Lobby Actions
                    card.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        flex_wrap: FlexWrap::Wrap,
                        column_gap: Val::Px(8.0),
                        row_gap: Val::Px(6.0),
                        margin: UiRect::top(Val::Px(6.0)),
                        ..default()
                    })
                    .with_children(|row| {
                        spawn_accent_button(row, "Play Solo", UiAction::PlaySolo);
                        spawn_accent_button(row, "Host Co-op Lobby", UiAction::HostLobby);
                        spawn_std_button(
                            row,
                            "Paste & Join Code",
                            UiAction::PasteAndJoin,
                        );
                        spawn_std_button(row, "Cancel", UiAction::CancelConnecting);
                    });

                    card.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(8.0),
                        ..default()
                    })
                    .with_children(|row| {
                        spawn_std_button(
                            row,
                            "Settings (FOV / Fullscreen / Audio)",
                            UiAction::OpenSettings,
                        );
                        spawn_danger_button(row, "Quit Game", UiAction::QuitGame);
                    });

                    // Status Line
                    card.spawn((
                        Text::new("Ready — Play Solo, Host, or Paste a friend's ECHO-... code"),
                        TextFont {
                            font_size: FontSize::Px(13.0),
                            ..default()
                        },
                        TextColor(Color::srgb(0.95, 0.84, 0.42)),
                        MainMenuStatusText,
                    ));
                });
        });

    // =========================================================================
    // 2. UNIFIED SETTINGS PANEL (FOV, Fullscreen, Audio I/O Devices & Sliders)
    // =========================================================================
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.03, 0.05, 0.78)),
            Visibility::Hidden,
            SettingsPanelRoot,
        ))
        .with_children(|backdrop| {
            backdrop
                .spawn((
                    Node {
                        width: Val::Px(620.0),
                        padding: UiRect::all(Val::Px(22.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(10.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.05, 0.08, 0.12, 0.97)),
                ))
                .with_children(|panel| {
                    panel.spawn((
                        Text::new("SETTINGS — VIDEO & AUDIO"),
                        TextFont {
                            font_size: FontSize::Px(18.0),
                            ..default()
                        },
                        TextColor(Color::srgb(0.30, 0.92, 0.86)),
                    ));

                    // Video section
                    spawn_slider_row(panel, "Field of View (FOV)", SliderKind::Fov);

                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::SpaceBetween,
                            width: Val::Percent(100.0),
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn((
                                Text::new("Display Mode"),
                                TextFont {
                                    font_size: FontSize::Px(13.5),
                                    ..default()
                                },
                                TextColor(Color::srgb(0.86, 0.91, 0.94)),
                            ));
                            let lbl_ent = spawn_std_button(
                                row,
                                "Windowed",
                                UiAction::ToggleFullscreen,
                            );
                            row.commands().entity(lbl_ent).insert(FullscreenButtonLabel);
                        });

                    // Audio Output Device row
                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::SpaceBetween,
                            column_gap: Val::Px(6.0),
                            width: Val::Percent(100.0),
                            margin: UiRect::top(Val::Px(6.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn((
                                Text::new("Audio Output"),
                                TextFont {
                                    font_size: FontSize::Px(13.5),
                                    ..default()
                                },
                                TextColor(Color::srgb(0.86, 0.91, 0.94)),
                                Node {
                                    width: Val::Px(120.0),
                                    ..default()
                                },
                            ));
                            row.spawn(Node {
                                flex_direction: FlexDirection::Row,
                                align_items: AlignItems::Center,
                                column_gap: Val::Px(6.0),
                                ..default()
                            })
                            .with_children(|ctrl| {
                                spawn_std_button(ctrl, "<", UiAction::PrevOutputDevice);
                                ctrl.spawn((
                                    Text::new("System Default"),
                                    TextFont {
                                        font_size: FontSize::Px(12.5),
                                        ..default()
                                    },
                                    TextColor(Color::srgb(0.95, 0.96, 0.98)),
                                    Node {
                                        width: Val::Px(280.0),
                                        ..default()
                                    },
                                    OutputDeviceLabel,
                                ));
                                spawn_std_button(ctrl, ">", UiAction::NextOutputDevice);
                            });
                        });

                    spawn_slider_row(panel, "Master Output Vol", SliderKind::MasterVolume);
                    spawn_slider_row(panel, "Voice Chat Vol", SliderKind::VoiceVolume);

                    // Audio Input Device row
                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::SpaceBetween,
                            column_gap: Val::Px(6.0),
                            width: Val::Percent(100.0),
                            margin: UiRect::top(Val::Px(6.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn((
                                Text::new("Mic Input"),
                                TextFont {
                                    font_size: FontSize::Px(13.5),
                                    ..default()
                                },
                                TextColor(Color::srgb(0.86, 0.91, 0.94)),
                                Node {
                                    width: Val::Px(120.0),
                                    ..default()
                                },
                            ));
                            row.spawn(Node {
                                flex_direction: FlexDirection::Row,
                                align_items: AlignItems::Center,
                                column_gap: Val::Px(6.0),
                                ..default()
                            })
                            .with_children(|ctrl| {
                                spawn_std_button(ctrl, "<", UiAction::PrevInputDevice);
                                ctrl.spawn((
                                    Text::new("System Default"),
                                    TextFont {
                                        font_size: FontSize::Px(12.5),
                                        ..default()
                                    },
                                    TextColor(Color::srgb(0.95, 0.96, 0.98)),
                                    Node {
                                        width: Val::Px(280.0),
                                        ..default()
                                    },
                                    InputDeviceLabel,
                                ));
                                spawn_std_button(ctrl, ">", UiAction::NextInputDevice);
                            });
                        });

                    spawn_slider_row(panel, "Mic Input Gain", SliderKind::MicGain);
                    spawn_slider_row(panel, "Mic Noise Gate", SliderKind::MicGate);

                    panel.spawn((
                        Text::new("Mic Meter: ..."),
                        TextFont {
                            font_size: FontSize::Px(13.0),
                            ..default()
                        },
                        TextColor(Color::srgb(0.88, 0.94, 0.92)),
                        SettingsMicStatusText,
                    ));

                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: Val::Px(6.0),
                            margin: UiRect::top(Val::Px(6.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            spawn_std_button(
                                row,
                                "Refresh Audio Devices",
                                UiAction::RefreshAudioDevices,
                            );
                            spawn_std_button(row, "Mute Mic", UiAction::ToggleMute);
                            spawn_std_button(row, "Self-Echo Test", UiAction::ToggleSelfEcho);
                            spawn_accent_button(row, "Back / Done", UiAction::CloseSettings);
                        });
                });
        });

    // =========================================================================
    // 3. IN-GAME HUD (Hidden on Main Menu)
    // =========================================================================
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            Visibility::Hidden,
            InGameHudRoot,
        ))
        .with_children(|hud| {
            // Damage / Downed subtle red screen tint overlay
            hud.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.85, 0.08, 0.08, 0.0)),
                DamageFlashOverlay,
            ));

            // Top-left live mission, health, mic & acoustic telemetry
            hud.spawn((
                Text::new("Initializing..."),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(Color::srgba(0.86, 0.94, 0.92, 0.92)),
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(12.0),
                    left: Val::Px(14.0),
                    ..default()
                },
                HudText,
            ));

            // Center crosshair dot
            hud.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(50.0),
                    top: Val::Percent(50.0),
                    width: Val::Px(4.0),
                    height: Val::Px(4.0),
                    margin: UiRect {
                        left: Val::Px(-2.0),
                        top: Val::Px(-2.0),
                        ..default()
                    },
                    ..default()
                },
                BackgroundColor(Color::srgba(0.95, 0.95, 0.95, 0.72)),
                CrosshairNode,
            ));

            // Bottom center stack: Prompt, Toast, Battery Pouch Selector, Active Belt Bar
            hud.spawn(Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(16.0),
                left: Val::Percent(0.0),
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|bottom| {
                bottom.spawn((
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(15.0),
                        ..default()
                    },
                    TextColor(Color::srgb(1.0, 0.88, 0.42)),
                    PromptText,
                ));

                bottom.spawn((
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(14.0),
                        ..default()
                    },
                    TextColor(Color::srgb(0.32, 0.95, 0.88)),
                    ToastText,
                ));

                bottom
                    .spawn((
                        Node {
                            padding: UiRect::axes(Val::Px(14.0), Val::Px(7.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.06, 0.12, 0.16, 0.92)),
                        Visibility::Hidden,
                        BatterySelectorBox,
                    ))
                    .with_children(|b| {
                        b.spawn((
                            Text::new(""),
                            TextFont {
                                font_size: FontSize::Px(14.0),
                                ..default()
                            },
                            TextColor(Color::srgb(0.42, 0.95, 1.0)),
                            BatterySelectorText,
                        ));
                    });

                bottom
                    .spawn((
                        Node {
                            padding: UiRect::axes(Val::Px(14.0), Val::Px(6.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.04, 0.06, 0.09, 0.82)),
                    ))
                    .with_children(|bar| {
                        bar.spawn((
                            Text::new(""),
                            TextFont {
                                font_size: FontSize::Px(13.5),
                                ..default()
                            },
                            TextColor(Color::srgb(0.90, 0.95, 0.94)),
                            BeltHudText,
                        ));
                    });
            });
        });

    // =========================================================================
    // 4. IN-GAME PAUSE / CO-OP MENU ([Tab] / [Esc])
    // =========================================================================
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                width: Val::Px(560.0),
                margin: UiRect {
                    left: Val::Px(-280.0),
                    top: Val::Px(-185.0),
                    ..default()
                },
                padding: UiRect::all(Val::Px(18.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(10.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.05, 0.07, 0.10, 0.95)),
            Visibility::Hidden,
            CoopPanelRoot,
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new("EXPEDITION MENU — CO-OP & CONTROLS"),
                TextFont {
                    font_size: FontSize::Px(16.5),
                    ..default()
                },
                TextColor(Color::srgb(0.30, 0.92, 0.86)),
            ));

            panel.spawn((
                Text::new("Status: Offline"),
                TextFont {
                    font_size: FontSize::Px(13.5),
                    ..default()
                },
                TextColor(Color::srgb(0.88, 0.92, 0.95)),
                CoopPanelStatusText,
            ));

            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: Val::Px(6.0),
                    row_gap: Val::Px(4.0),
                    ..default()
                })
                .with_children(|row| {
                    spawn_std_button(row, "Copy Join Code", UiAction::CopyJoinCode);
                    spawn_std_button(row, "Cycle Visor Color", UiAction::CycleVisor);
                    spawn_std_button(row, "Mute Mic (M)", UiAction::ToggleMute);
                    spawn_std_button(row, "Self-Echo Test (V)", UiAction::ToggleSelfEcho);
                });

            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: Val::Px(6.0),
                    row_gap: Val::Px(4.0),
                    ..default()
                })
                .with_children(|row| {
                    spawn_std_button(
                        row,
                        "Settings (FOV / Audio / Fullscreen)",
                        UiAction::OpenSettings,
                    );
                    spawn_accent_button(row, "Resume [Tab/Esc]", UiAction::ResumeGame);
                    spawn_danger_button(
                        row,
                        "Disconnect & Main Menu",
                        UiAction::DisconnectToMenu,
                    );
                });
        });

    // =========================================================================
    // 5. BASE SHED SUPPLY SHOP & DEPTH SELECTOR ([E] at Terminal / [B] in Shed)
    // =========================================================================
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                width: Val::Px(640.0),
                margin: UiRect {
                    left: Val::Px(-320.0),
                    top: Val::Px(-250.0),
                    ..default()
                },
                padding: UiRect::all(Val::Px(18.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.05, 0.08, 0.11, 0.96)),
            Visibility::Hidden,
            ShopPanelRoot,
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new("BASE SHED SUPPLY TERMINAL & DEPTH SELECTOR"),
                TextFont {
                    font_size: FontSize::Px(16.5),
                    ..default()
                },
                TextColor(Color::srgb(0.25, 0.96, 0.58)),
            ));

            panel.spawn((
                Text::new("Credits: 100 cr"),
                TextFont {
                    font_size: FontSize::Px(13.5),
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.90, 0.55)),
                ShopHeaderText,
            ));

            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(6.0),
                    ..default()
                })
                .with_children(|row| {
                    spawn_std_button(row, "Depth Tier 1", UiAction::ShopTier(1));
                    spawn_std_button(row, "Depth Tier 2", UiAction::ShopTier(2));
                    spawn_std_button(row, "Depth Tier 3", UiAction::ShopTier(3));
                    spawn_accent_button(row, "Save Campaign", UiAction::ShopManualSave);
                    spawn_danger_button(row, "Close [Esc/B]", UiAction::ShopClose);
                });

            panel
                .spawn(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    margin: UiRect::top(Val::Px(4.0)),
                    ..default()
                })
                .with_children(|list| {
                    for entry in SHOP_CATALOG {
                        let label = format!(
                            "{} ({} cr) — {}",
                            entry.name, entry.cost, entry.description
                        );
                        spawn_std_button(list, &label, UiAction::ShopBuy(entry.item));
                    }
                });
        });
}

fn handle_name_typing_system(
    screen: Res<AppScreen>,
    settings_open: Res<SettingsMenuOpen>,
    mut focused: ResMut<NameInputFocused>,
    mut key_events: MessageReader<KeyboardInput>,
    mut user_settings: ResMut<UserSettings>,
    mut coop: ResMut<CoopState>,
) {
    if *screen != AppScreen::MainMenu || settings_open.0 || !focused.0 {
        key_events.clear();
        return;
    }

    let mut changed = false;
    for ev in key_events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::Backspace => {
                if !user_settings.player_name.is_empty() {
                    user_settings.player_name.pop();
                    changed = true;
                }
            }
            Key::Enter | Key::Escape => {
                focused.0 = false;
            }
            Key::Space => {
                if user_settings.player_name.len() < 20 {
                    user_settings.player_name.push(' ');
                    changed = true;
                }
            }
            Key::Character(s) => {
                for ch in s.chars() {
                    if user_settings.player_name.len() < 20
                        && (ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == ' ')
                    {
                        user_settings.player_name.push(ch);
                        changed = true;
                    }
                }
            }
            _ => {}
        }
    }

    if changed {
        let trimmed = user_settings.player_name.trim();
        coop.local_name = if trimmed.is_empty() {
            "Miner".to_string()
        } else {
            trimmed.to_string()
        };
    }
}

fn apply_slider_step(settings: &mut UserSettings, kind: SliderKind, dir: f32) {
    match kind {
        SliderKind::Fov => {
            settings.fov_deg = (settings.fov_deg + dir * 5.0).clamp(FOV_MIN_DEG, FOV_MAX_DEG);
        }
        SliderKind::MasterVolume => {
            settings.master_volume = (settings.master_volume + dir * 0.10).clamp(0.0, 1.5);
        }
        SliderKind::VoiceVolume => {
            settings.voice_volume = (settings.voice_volume + dir * 0.10).clamp(0.0, 2.0);
        }
        SliderKind::MicGain => {
            settings.mic_gain = (settings.mic_gain + dir * 0.15).clamp(0.2, 3.0);
        }
        SliderKind::MicGate => {
            settings.gate_threshold_db =
                (settings.gate_threshold_db + dir * 3.0).clamp(-65.0, -15.0);
        }
    }
}

fn apply_slider_frac(settings: &mut UserSettings, kind: SliderKind, frac: f32) {
    let t = frac.clamp(0.0, 1.0);
    match kind {
        SliderKind::Fov => {
            settings.fov_deg = (FOV_MIN_DEG + t * (FOV_MAX_DEG - FOV_MIN_DEG)).round();
        }
        SliderKind::MasterVolume => {
            settings.master_volume = (t * 1.5 * 20.0).round() / 20.0;
        }
        SliderKind::VoiceVolume => {
            settings.voice_volume = (t * 2.0 * 20.0).round() / 20.0;
        }
        SliderKind::MicGain => {
            settings.mic_gain = ((0.2 + t * 2.8) * 20.0).round() / 20.0;
        }
        SliderKind::MicGate => {
            settings.gate_threshold_db = (-65.0 + t * 50.0).round();
        }
    }
}

fn slider_current_frac(settings: &UserSettings, kind: SliderKind) -> f32 {
    match kind {
        SliderKind::Fov => ((settings.fov_deg - FOV_MIN_DEG) / (FOV_MAX_DEG - FOV_MIN_DEG))
            .clamp(0.0, 1.0),
        SliderKind::MasterVolume => (settings.master_volume / 1.5).clamp(0.0, 1.0),
        SliderKind::VoiceVolume => (settings.voice_volume / 2.0).clamp(0.0, 1.0),
        SliderKind::MicGain => ((settings.mic_gain - 0.2) / 2.8).clamp(0.0, 1.0),
        SliderKind::MicGate => ((settings.gate_threshold_db + 65.0) / 50.0).clamp(0.0, 1.0),
    }
}

fn cycle_device(current: &Option<String>, list: &[String], delta: i32) -> Option<String> {
    let total = list.len() + 1; // index 0 = System Default (None)
    let cur_idx = match current {
        None => 0usize,
        Some(name) => list
            .iter()
            .position(|d| d == name)
            .map_or(0, |idx| idx + 1),
    };
    let next_idx = (cur_idx as i32 + delta).rem_euclid(total as i32) as usize;
    if next_idx == 0 {
        None
    } else {
        list.get(next_idx - 1).cloned()
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_ui_actions_system(
    mut commands: Commands,
    mut actions: MessageReader<TriggerUiAction>,
    mut menu_open: ResMut<CoopMenuOpen>,
    mut settings_open: ResMut<SettingsMenuOpen>,
    mut name_focused: ResMut<NameInputFocused>,
    mut coop: ResMut<CoopState>,
    mut user_settings: ResMut<UserSettings>,
    mut voice_settings: ResMut<VoiceSettings>,
    mut device_catalog: ResMut<AudioDeviceCatalog>,
    inv: Res<PlayerInventory>,
    mut lifecycle_writer: MessageWriter<SessionLifecycleEvent>,
    mut shop_writer: MessageWriter<ShopActionEvent>,
    mut remove_voice: MessageWriter<RemoveVoiceSpeakerEvent>,
    mut exit_writer: MessageWriter<AppExit>,
) {
    for TriggerUiAction(action) in actions.read().copied() {
        if !matches!(action, UiAction::FocusNameInput) {
            name_focused.0 = false;
        }
        match action {
            UiAction::FocusNameInput => {
                name_focused.0 = true;
            }
            UiAction::SelectVisor(idx) => {
                let clamped = idx % VISOR_PALETTE.len();
                user_settings.visor_idx = clamped;
                coop.visor_idx = clamped;
                if coop.is_connected() {
                    coop.sync_local_profile(&inv);
                }
            }
            UiAction::PlaySolo => {
                user_settings.sanitize();
                coop.local_name = user_settings.player_name.clone();
                coop.visor_idx = user_settings.visor_idx % VISOR_PALETTE.len();
                lifecycle_writer.write(SessionLifecycleEvent::StartSolo);
            }
            UiAction::HostLobby => {
                user_settings.sanitize();
                coop.local_name = user_settings.player_name.clone();
                coop.visor_idx = user_settings.visor_idx % VISOR_PALETTE.len();
                lifecycle_writer.write(SessionLifecycleEvent::StartHost);
            }
            UiAction::PasteAndJoin => {
                user_settings.sanitize();
                coop.local_name = user_settings.player_name.clone();
                coop.visor_idx = user_settings.visor_idx % VISOR_PALETTE.len();
                clear_remote_players(&mut commands, &mut coop, &mut remove_voice);
                if let Some(code) = paste_from_clipboard() {
                    coop.start_join(code);
                } else {
                    coop.status_text =
                        "Clipboard empty — copy host's ECHO-... code first!".into();
                }
            }
            UiAction::CancelConnecting => {
                clear_remote_players(&mut commands, &mut coop, &mut remove_voice);
                coop.session = None;
                coop.join_code = None;
                coop.pending_disconnect_s = None;
                coop.status_text = "Cancelled — Ready".into();
            }
            UiAction::OpenSettings => {
                device_catalog.refresh();
                settings_open.0 = true;
            }
            UiAction::CloseSettings => {
                settings_open.0 = false;
                let _ = user_settings.save_default();
            }
            UiAction::QuitGame => {
                let _ = user_settings.save_default();
                exit_writer.write(AppExit::Success);
            }
            UiAction::CopyJoinCode => {
                if let Some(code) = coop.join_code.clone() {
                    if copy_to_clipboard(&code) {
                        coop.status_text = "Join code copied to clipboard!".into();
                    } else {
                        coop.status_text = "Could not access clipboard.".into();
                    }
                } else {
                    coop.status_text = "Host a lobby to generate a join code.".into();
                }
            }
            UiAction::DisconnectToMenu => {
                lifecycle_writer.write(SessionLifecycleEvent::DisconnectToMenu);
            }
            UiAction::CycleVisor => {
                let next = (coop.visor_idx + 1) % VISOR_PALETTE.len();
                coop.visor_idx = next;
                user_settings.visor_idx = next;
                // Broadcast visor color change immediately to all peers!
                if coop.is_connected() {
                    coop.sync_local_profile(&inv);
                }
            }
            UiAction::ToggleMute => {
                voice_settings.muted = !voice_settings.muted;
            }
            UiAction::ToggleSelfEcho => {
                voice_settings.self_echo_test = !voice_settings.self_echo_test;
            }
            UiAction::ResumeGame => {
                menu_open.0 = false;
                settings_open.0 = false;
            }
            UiAction::ToggleFullscreen => {
                user_settings.fullscreen = !user_settings.fullscreen;
            }
            UiAction::PrevOutputDevice => {
                user_settings.output_device =
                    cycle_device(&user_settings.output_device, &device_catalog.output_devices, -1);
            }
            UiAction::NextOutputDevice => {
                user_settings.output_device =
                    cycle_device(&user_settings.output_device, &device_catalog.output_devices, 1);
            }
            UiAction::PrevInputDevice => {
                user_settings.input_device =
                    cycle_device(&user_settings.input_device, &device_catalog.input_devices, -1);
            }
            UiAction::NextInputDevice => {
                user_settings.input_device =
                    cycle_device(&user_settings.input_device, &device_catalog.input_devices, 1);
            }
            UiAction::RefreshAudioDevices => {
                device_catalog.refresh();
            }
            UiAction::SliderStep(kind, dir) => {
                apply_slider_step(&mut user_settings, kind, dir);
            }
            UiAction::SliderSetFrac(kind, frac) => {
                apply_slider_frac(&mut user_settings, kind, frac);
            }
            UiAction::ShopBuy(item) => {
                shop_writer.write(ShopActionEvent::Buy(item));
            }
            UiAction::ShopTier(tier) => {
                shop_writer.write(ShopActionEvent::SelectDepthTier(tier));
            }
            UiAction::ShopManualSave => {
                shop_writer.write(ShopActionEvent::ManualSave);
            }
            UiAction::ShopClose => {
                shop_writer.write(ShopActionEvent::Close);
            }
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_panel_visibility_system(
    screen: Res<AppScreen>,
    menu_open: Res<CoopMenuOpen>,
    shop_open: Res<ShopOverlayOpen>,
    settings_open: Res<SettingsMenuOpen>,
    inv: Res<PlayerInventory>,
    mut main_menu_q: Query<
        &mut Visibility,
        (
            With<MainMenuRoot>,
            Without<SettingsPanelRoot>,
            Without<InGameHudRoot>,
            Without<CoopPanelRoot>,
            Without<ShopPanelRoot>,
            Without<CrosshairNode>,
            Without<BatterySelectorBox>,
        ),
    >,
    mut settings_q: Query<
        &mut Visibility,
        (
            With<SettingsPanelRoot>,
            Without<MainMenuRoot>,
            Without<InGameHudRoot>,
            Without<CoopPanelRoot>,
            Without<ShopPanelRoot>,
            Without<CrosshairNode>,
            Without<BatterySelectorBox>,
        ),
    >,
    mut hud_q: Query<
        &mut Visibility,
        (
            With<InGameHudRoot>,
            Without<MainMenuRoot>,
            Without<SettingsPanelRoot>,
            Without<CoopPanelRoot>,
            Without<ShopPanelRoot>,
            Without<CrosshairNode>,
            Without<BatterySelectorBox>,
        ),
    >,
    mut coop_q: Query<
        &mut Visibility,
        (
            With<CoopPanelRoot>,
            Without<MainMenuRoot>,
            Without<SettingsPanelRoot>,
            Without<InGameHudRoot>,
            Without<ShopPanelRoot>,
            Without<CrosshairNode>,
            Without<BatterySelectorBox>,
        ),
    >,
    mut shop_q: Query<
        &mut Visibility,
        (
            With<ShopPanelRoot>,
            Without<MainMenuRoot>,
            Without<SettingsPanelRoot>,
            Without<InGameHudRoot>,
            Without<CoopPanelRoot>,
            Without<CrosshairNode>,
            Without<BatterySelectorBox>,
        ),
    >,
    mut crosshair_q: Query<
        &mut Visibility,
        (
            With<CrosshairNode>,
            Without<MainMenuRoot>,
            Without<SettingsPanelRoot>,
            Without<InGameHudRoot>,
            Without<CoopPanelRoot>,
            Without<ShopPanelRoot>,
            Without<BatterySelectorBox>,
        ),
    >,
    mut bat_box_q: Query<
        &mut Visibility,
        (
            With<BatterySelectorBox>,
            Without<MainMenuRoot>,
            Without<SettingsPanelRoot>,
            Without<InGameHudRoot>,
            Without<CoopPanelRoot>,
            Without<ShopPanelRoot>,
            Without<CrosshairNode>,
        ),
    >,
) {
    let in_game = *screen == AppScreen::InGame;
    let vis = |show: bool| {
        if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        }
    };

    if let Ok(mut v) = main_menu_q.single_mut() {
        *v = vis(!in_game && !settings_open.0);
    }
    if let Ok(mut v) = settings_q.single_mut() {
        *v = vis(settings_open.0);
    }
    if let Ok(mut v) = hud_q.single_mut() {
        *v = vis(in_game);
    }
    if let Ok(mut v) = coop_q.single_mut() {
        *v = vis(in_game && menu_open.0 && !settings_open.0);
    }
    if let Ok(mut v) = shop_q.single_mut() {
        *v = vis(in_game && shop_open.0 && !settings_open.0);
    }
    if let Ok(mut v) = crosshair_q.single_mut() {
        *v = vis(in_game && !menu_open.0 && !shop_open.0 && !settings_open.0);
    }
    if let Ok(mut v) = bat_box_q.single_mut() {
        *v = vis(in_game && inv.battery_selector.open);
    }
}

fn format_db_meter(rms_db: f32, gate_db: f32) -> String {
    let slots = 20usize;
    let norm = |db: f32| ((db + 70.0) / 65.0).clamp(0.0, 1.0);
    let level_idx = (norm(rms_db) * slots as f32).round() as usize;
    let gate_idx = ((norm(gate_db) * slots as f32).round() as usize).min(slots - 1);

    let mut bar = String::with_capacity(slots + 2);
    bar.push('[');
    for i in 0..slots {
        if i == gate_idx {
            bar.push('|');
        } else if i < level_idx {
            bar.push('#');
        } else {
            bar.push('.');
        }
    }
    bar.push(']');
    bar
}

fn truncate_device_name(name: Option<&str>) -> String {
    match name {
        None => "System Default".to_string(),
        Some(s) if s.len() > 34 => format!("{}...", &s[..31]),
        Some(s) => s.to_string(),
    }
}

#[derive(SystemParam)]
struct MenuHudRead<'w> {
    telemetry: Res<'w, AcousticTelemetry>,
    coop: Res<'w, CoopState>,
    voice_settings: Res<'w, VoiceSettings>,
    user_settings: Res<'w, UserSettings>,
    name_focused: Res<'w, NameInputFocused>,
    campaign: Res<'w, CampaignState>,
}

#[allow(clippy::type_complexity)]
fn update_menu_hud_texts_system(
    st: MenuHudRead,
    mut menu_texts: ParamSet<(
        Query<&mut Text, With<MainMenuNameText>>,
        Query<&mut Text, With<MainMenuStatusText>>,
        Query<&mut Text, With<CoopPanelStatusText>>,
        Query<&mut Text, With<ShopHeaderText>>,
    )>,
    mut name_box_q: Query<&mut BackgroundColor, (With<MainMenuNameBox>, Without<VisorSwatchButton>)>,
    mut swatch_q: Query<(&VisorSwatchButton, &mut BackgroundColor), Without<MainMenuNameBox>>,
) {
    let telemetry = &st.telemetry;
    let coop = &st.coop;
    let voice_settings = &st.voice_settings;
    let user_settings = &st.user_settings;
    let name_focused = &st.name_focused;
    let campaign = &st.campaign;

    if let Ok(mut t) = menu_texts.p0().single_mut() {
        let cursor_ch = if name_focused.0 { "_" } else { "" };
        t.0 = format!("{}{}", user_settings.player_name, cursor_ch);
    }
    if let Ok(mut bg) = name_box_q.single_mut() {
        *bg = if name_focused.0 {
            BackgroundColor(Color::srgba(0.14, 0.28, 0.34, 0.98))
        } else {
            BackgroundColor(Color::srgba(0.11, 0.15, 0.21, 0.98))
        };
    }
    for (swatch, mut bg) in &mut swatch_q {
        if swatch.0 == coop.visor_idx % VISOR_PALETTE.len() {
            *bg = BackgroundColor(Color::srgba(0.24, 0.44, 0.50, 0.98));
        } else {
            *bg = BackgroundColor(Color::srgba(0.12, 0.16, 0.22, 0.94));
        }
    }
    if let Ok(mut t) = menu_texts.p1().single_mut() {
        t.0 = coop.status_text.clone();
    }

    let mic = telemetry.mic;
    let mic_bar = format_db_meter(mic.rms_db, voice_settings.gate_threshold_db);
    let voice_state = format_voice_state(mic, voice_settings);
    let echo_tag = if voice_settings.self_echo_test {
        "ON"
    } else {
        "OFF"
    };

    if let Ok(mut t) = menu_texts.p2().single_mut() {
        let code_preview = match &coop.join_code {
            Some(c) if c.len() > 44 => {
                format!("{}...{} ({} chars)", &c[..24], &c[c.len() - 12..], c.len())
            }
            Some(c) => c.clone(),
            None => "Offline / Solo".into(),
        };
        let peers: Vec<String> = coop
            .remote_players
            .values()
            .map(|rp| rp.info.name.clone())
            .collect();
        let peer_str = if peers.is_empty() {
            "None".to_string()
        } else {
            peers.join(", ")
        };
        t.0 = format!(
            "Session: {}\n\
             Join Code: {}\n\
             Miner: {} (Visor: {})  |  Teammates: {}\n\
             Mic: {} {:.0} dB (Gate {:.0} dB) — {}  |  Self-Echo: {}",
            coop.status_text,
            code_preview,
            coop.local_name,
            coop.visor_label(),
            peer_str,
            mic_bar,
            mic.rms_db,
            voice_settings.gate_threshold_db,
            voice_state,
            echo_tag,
        );
    }

    if let Ok(mut t) = menu_texts.p3().single_mut() {
        t.0 = format!(
            "Credits: {} cr  |  Selected Depth: Tier {} (Unlocked: Tier {})  |  Dives Completed: {}",
            campaign.save.credits,
            campaign.save.selected_depth_tier,
            campaign.save.unlocked_depth_tier,
            campaign.save.dives_completed,
        );
    }
}

fn format_voice_state(
    mic: voxel_audio_dsp::MicTelemetry,
    voice_settings: &VoiceSettings,
) -> &'static str {
    if !mic.available {
        "NO MIC"
    } else if voice_settings.muted {
        "MUTED"
    } else if !mic.gate_open {
        "SILENT"
    } else if mic.ai_noise_level < 0.35 {
        "WHISPER (safe)"
    } else if mic.ai_noise_level < 1.2 {
        "TALKING"
    } else {
        "LOUD! (alerts AI)"
    }
}

#[derive(SystemParam)]
struct InGameHudRead<'w> {
    telemetry: Res<'w, AcousticTelemetry>,
    coop: Res<'w, CoopState>,
    voice_settings: Res<'w, VoiceSettings>,
    elevator: Res<'w, ElevatorState>,
    campaign: Res<'w, CampaignState>,
    inv: Res<'w, PlayerInventory>,
    health: Res<'w, PlayerHealthState>,
    station_state: Res<'w, ShedStationState>,
    item_reg: Res<'w, WorldItemRegistry>,
    toast: Res<'w, GameplayToast>,
}

#[allow(clippy::type_complexity)]
fn update_ingame_hud_texts_system(
    st: InGameHudRead,
    fl_q: Query<&PlayerFlashlight>,
    mut ingame_texts: ParamSet<(
        Query<&mut Text, With<HudText>>,
        Query<&mut Text, With<PromptText>>,
        Query<&mut Text, With<ToastText>>,
        Query<&mut Text, With<BatterySelectorText>>,
        Query<&mut Text, With<BeltHudText>>,
    )>,
    mut flash_q: Query<&mut BackgroundColor, With<DamageFlashOverlay>>,
) {
    let telemetry = &st.telemetry;
    let coop = &st.coop;
    let voice_settings = &st.voice_settings;
    let elevator = &st.elevator;
    let campaign = &st.campaign;
    let inv = &st.inv;
    let health = &st.health;
    let station_state = &st.station_state;
    let item_reg = &st.item_reg;
    let toast = &st.toast;

    let mic = telemetry.mic;
    let mic_bar = format_db_meter(mic.rms_db, voice_settings.gate_threshold_db);
    let voice_state = format_voice_state(mic, voice_settings);
    let echo_tag = if voice_settings.self_echo_test {
        "ON"
    } else {
        "OFF"
    };

    let battery_pct = fl_q
        .single()
        .map(|f| (f.battery * 100.0).round() as i32)
        .unwrap_or(0);
    let fl_state = fl_q
        .single()
        .map(|f| if f.on { "ON" } else { "OFF" })
        .unwrap_or("OFF");
    let rev = telemetry.reverb;
    let beacon_line = if let Some(b) = telemetry.beacon_params {
        let active_taps = b.early_taps.iter().filter(|t| t.active).count();
        format!(
            "Beacon: {:.1}m | Wall: {:.2}m | Gain L/M/H: {:.0}/{:.0}/{:.0} dB | Taps: {}",
            b.distance_m,
            b.solid_thickness_m,
            linear_to_db(b.direct_gain.0[0]),
            linear_to_db(b.direct_gain.0[1]),
            linear_to_db(b.direct_gain.0[2]),
            active_taps,
        )
    } else {
        "Beacon: solving...".to_string()
    };

    if let Ok(mut t) = ingame_texts.p0().single_mut() {
        t.0 = format!(
            "ECHOES BELOW  |  {} (Visor: {})  |  Credits: {} cr  |  Tier {}/{}  |  [Tab/Esc] Menu\n\
             Status: {}  |  Health: {}\n\
             Mic: {} {:.0} dB (Gate {:.0} dB) — {}  |  Self-Echo [V]: {}\n\
             Room RT60 L/M/H: {:.2}/{:.2}/{:.2}s | MFP {:.1}m | {}\n\
             Light: {} ({}%) [F] | [R] Battery | [Q] Drop | [G] Flare | [T] Decoy | [LMB] Break | [RMB] Foam | [B] Shop",
            coop.local_name,
            coop.visor_label(),
            campaign.save.credits,
            campaign.save.selected_depth_tier,
            campaign.save.unlocked_depth_tier,
            elevator.hud_status(),
            health.hud_label(),
            mic_bar,
            mic.rms_db,
            voice_settings.gate_threshold_db,
            voice_state,
            echo_tag,
            rev.rt60_sec.0[0],
            rev.rt60_sec.0[1],
            rev.rt60_sec.0[2],
            rev.mean_free_path_m,
            beacon_line,
            fl_state,
            battery_pct,
        );
    }

    if let Ok(mut t) = ingame_texts.p1().single_mut() {
        t.0 = station_state
            .prompt
            .clone()
            .or_else(|| item_reg.nearest_item_prompt.clone())
            .unwrap_or_default();
    }
    if let Ok(mut t) = ingame_texts.p2().single_mut() {
        t.0 = toast.text.clone();
    }
    if let Ok(mut t) = ingame_texts.p3().single_mut() {
        t.0 = inv.format_battery_selector_hud();
    }
    if let Ok(mut t) = ingame_texts.p4().single_mut() {
        t.0 = inv.format_belt_hud();
    }

    if let Ok(mut bg) = flash_q.single_mut() {
        let alpha = if health.is_dead() {
            0.42
        } else if health.is_downed() {
            (0.22 + health.damage_flash_s * 0.25).clamp(0.0, 0.5)
        } else {
            (health.damage_flash_s * 0.35).clamp(0.0, 0.4)
        };
        *bg = BackgroundColor(Color::srgba(0.85, 0.08, 0.08, alpha));
    }
}

#[allow(clippy::type_complexity)]
fn update_settings_widgets_system(
    user_settings: Res<UserSettings>,
    voice_settings: Res<VoiceSettings>,
    telemetry: Res<AcousticTelemetry>,
    mut seg_q: Query<(&SliderSegment, &mut BackgroundColor)>,
    mut text_set: ParamSet<(
        Query<(&SliderValueText, &mut Text)>,
        Query<&mut Text, With<OutputDeviceLabel>>,
        Query<&mut Text, With<InputDeviceLabel>>,
        Query<&mut Text, With<FullscreenButtonLabel>>,
        Query<&mut Text, With<SettingsMicStatusText>>,
    )>,
) {
    for (seg, mut bg) in &mut seg_q {
        let frac = slider_current_frac(&user_settings, seg.kind);
        let filled_count = (frac * SLIDER_SEGMENTS as f32).round() as usize;
        *bg = if seg.idx < filled_count {
            BackgroundColor(Color::srgba(0.22, 0.85, 0.78, 0.98))
        } else {
            BackgroundColor(Color::srgba(0.14, 0.19, 0.25, 0.92))
        };
    }

    for (val_tag, mut t) in &mut text_set.p0() {
        t.0 = match val_tag.0 {
            SliderKind::Fov => format!("{:.0}°", user_settings.fov_deg),
            SliderKind::MasterVolume => format!("{:.0}%", user_settings.master_volume * 100.0),
            SliderKind::VoiceVolume => format!("{:.0}%", user_settings.voice_volume * 100.0),
            SliderKind::MicGain => format!("{:.0}%", user_settings.mic_gain * 100.0),
            SliderKind::MicGate => format!("{:.0} dB", user_settings.gate_threshold_db),
        };
    }

    if let Ok(mut t) = text_set.p1().single_mut() {
        t.0 = truncate_device_name(user_settings.output_device.as_deref());
    }
    if let Ok(mut t) = text_set.p2().single_mut() {
        t.0 = truncate_device_name(user_settings.input_device.as_deref());
    }
    if let Ok(mut t) = text_set.p3().single_mut() {
        t.0 = if user_settings.fullscreen {
            "Borderless Fullscreen".to_string()
        } else {
            "Windowed".to_string()
        };
    }
    if let Ok(mut t) = text_set.p4().single_mut() {
        let mic = telemetry.mic;
        let bar = format_db_meter(mic.rms_db, user_settings.gate_threshold_db);
        let state = if !mic.available {
            "NO MIC"
        } else if voice_settings.muted {
            "MUTED"
        } else if !mic.gate_open {
            "SILENT"
        } else if mic.ai_noise_level < 0.35 {
            "WHISPER"
        } else if mic.ai_noise_level < 1.2 {
            "TALKING"
        } else {
            "LOUD!"
        };
        t.0 = format!(
            "Live Mic: {} {:.0} dB  |  {}  |  Self-Echo: {}",
            bar,
            mic.rms_db,
            state,
            if voice_settings.self_echo_test {
                "ON"
            } else {
                "OFF"
            }
        );
    }
}
