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
use voxel_net::ElevatorPhase;
use voxel_updater::APP_VERSION;

use crate::{
    audio_bridge::{
        AcousticTelemetry, AudioDeviceCatalog, RemoveVoiceSpeakerEvent, VoiceSettings,
    },
    elevator::{
        ELEVATOR_ASCENT_DURATION_S, ELEVATOR_DESCENT_DURATION_S, ELEVATOR_WINCH_DURATION_S,
        ElevatorState,
    },
    gameplay::{GameplayToast, SessionLifecycleEvent, ShopActionEvent},
    health::{BLEEDOUT_DURATION_S, LocalHealthPhase, PlayerHealthState},
    inventory::{
        BeltItem, CarryHandState, MAX_BELT_CAPACITY, MAX_POUCH_CAPACITY, PlayerInventory,
        SHOP_CATALOG, ShopCatalogItem,
    },
    net_bridge::{
        CoopMenuOpen, CoopState, VISOR_PALETTE, clear_remote_players, copy_to_clipboard,
        paste_from_clipboard,
    },
    player::PlayerFlashlight,
    relic::{WorldItemEntity, world_item_display_name},
    save::{
        CampaignState, MAX_PARKOUR_LEADERBOARD, MAX_SAVE_SLOT_NAME_LEN, SaveSlotBrowser,
        validate_save_slot_name,
    },
    settings::{AppScreen, FOV_MAX_DEG, FOV_MIN_DEG, SettingsMenuOpen, UserSettings},
    shed::{
        CrosshairTargetState, ParkourTimerState, ShopOverlayOpen, TargetAccent,
        format_parkour_time_ms,
    },
    updater_bridge::{UpdaterCommand, UpdaterState, UpdaterUiStatus},
};

const SLIDER_HIT_STEPS: usize = 20;
const MIC_LED_BARS: usize = 10;
const SAVE_DROPDOWN_ROWS: usize = 6;
const AUDIO_DROPDOWN_ROWS: usize = 6;

// ---- Color Palette (Subterranean Tactical Dive UI) ----
const COL_PANEL_BG: Color = Color::srgba(0.04, 0.06, 0.09, 0.94);
const COL_CARD_BG: Color = Color::srgba(0.07, 0.10, 0.15, 0.95);
const COL_HUD_POD_BG: Color = Color::srgba(0.04, 0.06, 0.09, 0.84);
const COL_BORDER_DIM: Color = Color::srgba(0.20, 0.28, 0.36, 0.85);
const COL_ACCENT_CYAN: Color = Color::srgb(0.22, 0.92, 0.86);
const COL_ACCENT_AMBER: Color = Color::srgb(1.0, 0.74, 0.22);
const COL_ACCENT_GREEN: Color = Color::srgb(0.24, 0.92, 0.56);
const COL_ACCENT_RED: Color = Color::srgb(0.96, 0.26, 0.28);
const COL_TEXT_BRIGHT: Color = Color::srgb(0.94, 0.97, 0.99);
const COL_TEXT_MUTED: Color = Color::srgb(0.62, 0.72, 0.80);

#[derive(Resource, Default)]
struct NameInputFocused(bool);

#[derive(Resource, Default)]
struct DiagnosticsDrawerOpen(bool);

#[derive(Resource, Default)]
struct SaveDropdownState {
    open: bool,
    page: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveNamePromptMode {
    Create,
    Rename,
}

#[derive(Resource, Default)]
struct SaveNamePromptState {
    open: bool,
    focused: bool,
    caret_reset_s: f32,
    mode: Option<SaveNamePromptMode>,
    buffer: String,
    default_hint: String,
    target_old_name: String,
    error_text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioDropdownKind {
    Output,
    Input,
}

#[derive(Resource, Default)]
struct AudioDropdownState {
    open: Option<AudioDropdownKind>,
    page: usize,
}

// ---- Unified Panel Visibility Marker (Zero B0001 conflicts!) ----

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
enum UiPanelKind {
    MainMenu,
    Settings,
    SaveNameModal,
    UpdatePopup,
    InGameHud,
    CoopMenu,
    ShopOverlay,
    Crosshair,
    TargetCard,
    ToastCard,
    BatteryTray,
    ParkourHud,
    DiagDrawer,
}

// ---- Main Menu / Pause / Shop Markers ----

#[derive(Component)]
struct MainMenuNameBox;

#[derive(Component)]
struct MainMenuNameText;

#[derive(Component)]
struct MainMenuStatusText;

#[derive(Component)]
struct MainMenuSaveNameText;

#[derive(Component)]
struct MainMenuSaveMetaText;

#[derive(Component)]
struct SaveDropdownChevron;

#[derive(Component)]
struct SaveDropdownList;

#[derive(Component, Clone, Copy)]
struct SaveDropdownRow {
    idx: usize,
    hovered: bool,
}

#[derive(Component, Clone, Copy)]
struct SaveDropdownRowName(usize);

#[derive(Component, Clone, Copy)]
struct SaveDropdownRowMeta(usize);

#[derive(Component)]
struct SaveDropdownPagerRow;

#[derive(Component)]
struct SaveDropdownPageLabel;

#[derive(Component)]
struct SaveNameModalTitle;

#[derive(Component)]
struct SaveNameModalSubtitle;

#[derive(Component)]
struct SaveNameInputBox;

#[derive(Component)]
struct SaveNameFocusBadge;

#[derive(Component)]
struct SaveNameInputText;

#[derive(Component)]
struct SaveNameHintText;

#[derive(Component)]
struct SaveNameConfirmLabel;

#[derive(Component)]
struct ParkourClockText;

#[derive(Component)]
struct ParkourClockStatusText;

#[derive(Component, Clone, Copy)]
struct ParkourBoardRow(usize);

#[derive(Component, Clone, Copy)]
struct SaveTierPip(u8);

#[derive(Component, Clone, Copy)]
struct VisorHelmetButton(usize);

#[derive(Component)]
struct CoopPanelStatusText;

#[derive(Component)]
struct ShopCreditsText;

#[derive(Component)]
struct ShopSaveTagText;

#[derive(Component, Clone, Copy)]
struct ShopTierTab(u8);

// ---- Settings & Dynamic Toggle Button Markers ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToggleKind {
    Mute,
    SelfEcho,
    Fullscreen,
    IncludePrereleases,
}

#[derive(Component, Clone, Copy)]
struct ToggleButtonPill {
    kind: ToggleKind,
    hovered: bool,
}

#[derive(Component, Clone, Copy)]
struct ToggleButtonLabel(ToggleKind);

#[derive(Component)]
struct SettingsUpdaterStatusText;

#[derive(Component)]
struct UpdatePopupVersionText;

#[derive(Component)]
struct UpdatePopupNotesText;

#[derive(Component)]
struct UpdatePopupStatusText;

#[derive(Component)]
struct UpdatePopupProgressWrap;

#[derive(Component)]
struct UpdatePopupProgressFill;

#[derive(Component)]
struct UpdatePopupInstallLabel;

#[derive(Component)]
struct UpdatePopupDismissLabel;

#[derive(Component)]
struct OutputDeviceLabel;

#[derive(Component)]
struct InputDeviceLabel;

#[derive(Component, Clone, Copy)]
struct AudioDropdownChevron(AudioDropdownKind);

#[derive(Component, Clone, Copy)]
struct AudioDropdownList(AudioDropdownKind);

#[derive(Component, Clone, Copy)]
struct AudioDropdownRow {
    kind: AudioDropdownKind,
    idx: usize,
    hovered: bool,
}

#[derive(Component, Clone, Copy)]
struct AudioDropdownRowText {
    kind: AudioDropdownKind,
    idx: usize,
}

#[derive(Component, Clone, Copy)]
struct AudioDropdownPagerRow(AudioDropdownKind);

#[derive(Component, Clone, Copy)]
struct AudioDropdownPageLabel(AudioDropdownKind);

#[derive(Component)]
struct SettingsMicBadgeText;

#[derive(Component, Clone, Copy)]
struct SettingsMicLed(usize);

// ---- In-Game Graphical HUD Markers ----

#[derive(Component)]
struct DamageFlashOverlay;

#[derive(Component)]
struct HudTierBadgeText;

#[derive(Component)]
struct HudCreditsText;

#[derive(Component)]
struct HudPhaseText;

#[derive(Component)]
struct HudElevatorProgressFill;

#[derive(Component)]
struct HudHealthFill;

#[derive(Component)]
struct HudHealthBadgeText;

#[derive(Component, Clone, Copy)]
struct HudMicLed(usize);

#[derive(Component)]
struct HudMicStateDot;

#[derive(Component)]
struct HudBatteryFill;

#[derive(Component)]
struct HudBatteryLampDot;

#[derive(Component)]
struct CrosshairCenterDot;

#[derive(Component)]
struct CrosshairBracket;

#[derive(Component)]
struct TargetCardFrame;

#[derive(Component)]
struct TargetKeyBadgeBg;

#[derive(Component)]
struct TargetKeyBadgeText;

#[derive(Component)]
struct TargetLabelText;

#[derive(Component)]
struct TargetSublabelText;

#[derive(Component)]
struct ToastText;

#[derive(Component)]
struct DiagText;

// ---- Graphical Hotbar, Pouch & Battery Tray Markers ----

#[derive(Component)]
struct HotbarActiveNameText;

#[derive(Component, Clone, Copy)]
struct HotbarSlotCard(usize);

#[derive(Component, Clone, Copy)]
struct HotbarIconPrimary(usize);

#[derive(Component, Clone, Copy)]
struct HotbarIconSecondary(usize);

#[derive(Component, Clone, Copy)]
struct HotbarCountText(usize);

#[derive(Component, Clone, Copy)]
struct HotbarChargeFill(usize);

#[derive(Component, Clone, Copy)]
struct PouchGaugeFill(usize);

#[derive(Component, Clone, Copy)]
struct WearableBadge {
    is_headlamp: bool,
}

#[derive(Component)]
struct HeadlampBadgeText;

#[derive(Component, Clone, Copy)]
struct TrayCellCard(usize);

#[derive(Component, Clone, Copy)]
struct TrayCellFill(usize);

#[derive(Component, Clone, Copy)]
struct TrayCellPctText(usize);

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
struct SliderFillBar(SliderKind);

#[derive(Component, Clone, Copy, Debug)]
enum UiAction {
    PlaySolo,
    HostLobby,
    PasteAndJoin,
    CancelConnecting,
    FocusNameInput,
    SelectVisor(usize),
    ToggleSaveDropdown,
    SelectSaveRow(usize),
    SaveListPage(i32),
    OpenNewSavePrompt,
    OpenRenameSavePrompt,
    FocusSaveNameInput,
    BlurSaveNameInput,
    ConfirmSaveNamePrompt,
    CancelSaveNamePrompt,
    RefreshSaveSlots,
    OpenSettings,
    CloseSettings,
    QuitGame,

    CopyJoinCode,
    DisconnectToMenu,
    QuitNoSave,
    CycleVisor,
    ToggleMute,
    ToggleSelfEcho,
    ResumeGame,

    ToggleFullscreen,
    ToggleIncludePrereleases,
    CheckForUpdates,
    UpdateInstall,
    UpdateDismiss,
    ToggleAudioDropdown(AudioDropdownKind),
    SelectAudioRow(AudioDropdownKind, usize),
    AudioListPage(i32),
    RefreshAudioDevices,
    SliderStep(SliderKind, f32),
    SliderSetFrac(SliderKind, f32),

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
            .init_resource::<DiagnosticsDrawerOpen>()
            .init_resource::<SaveDropdownState>()
            .init_resource::<SaveNamePromptState>()
            .init_resource::<AudioDropdownState>()
            .add_message::<TriggerUiAction>()
            .add_systems(Startup, setup_hud)
            .add_systems(
                Update,
                (
                    handle_name_typing_system,
                    handle_ui_actions_system,
                    update_panel_visibility_system,
                    update_menu_and_shop_system,
                    update_save_dropdown_system,
                    update_save_name_modal_system,
                    update_parkour_hud_system,
                    update_ingame_bars_system,
                    update_crosshair_and_target_system,
                    update_hotbar_and_pouch_system,
                    update_battery_tray_system,
                    update_settings_widgets_system,
                    update_audio_dropdown_system,
                    update_updater_popup_system,
                    update_diagnostics_drawer_system,
                )
                    .chain(),
            );
    }
}

// =============================================================================
// UI BUILDER HELPERS
// =============================================================================

fn spawn_pill_button(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    action: UiAction,
    base_color: Color,
    hover_color: Color,
    border_color: Color,
) -> Entity {
    let mut text_ent = Entity::PLACEHOLDER;
    parent
        .spawn((
            Node {
                padding: UiRect::axes(Val::Px(12.0), Val::Px(7.0)),
                border: UiRect::all(Val::Px(1.5)),
                border_radius: BorderRadius::all(Val::Px(6.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(base_color),
            BorderColor::all(border_color),
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
            text_ent = btn
                .spawn((
                    Text::new(label),
                    TextFont {
                        font_size: FontSize::Px(13.0),
                        ..default()
                    },
                    TextColor(COL_TEXT_BRIGHT),
                    Pickable::IGNORE,
                ))
                .id();
        });
    text_ent
}

/// Interactive toggle pill whose background, border, and label dynamically reflect live ON/OFF state.
fn spawn_toggle_btn(
    parent: &mut ChildSpawnerCommands,
    initial_label: &str,
    action: UiAction,
    kind: ToggleKind,
) {
    parent
        .spawn((
            Node {
                padding: UiRect::axes(Val::Px(12.0), Val::Px(7.0)),
                border: UiRect::all(Val::Px(1.5)),
                border_radius: BorderRadius::all(Val::Px(6.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.11, 0.16, 0.22, 0.96)),
            BorderColor::all(COL_BORDER_DIM),
            action,
            ToggleButtonPill {
                kind,
                hovered: false,
            },
        ))
        .observe(
            move |_click: On<PointerClick>, mut writer: MessageWriter<TriggerUiAction>| {
                writer.write(TriggerUiAction(action));
            },
        )
        .observe(
            |over: On<PointerOver>, mut pill_q: Query<&mut ToggleButtonPill>| {
                if let Ok(mut pill) = pill_q.get_mut(over.entity) {
                    pill.hovered = true;
                }
            },
        )
        .observe(
            |out: On<PointerOut>, mut pill_q: Query<&mut ToggleButtonPill>| {
                if let Ok(mut pill) = pill_q.get_mut(out.entity) {
                    pill.hovered = false;
                }
            },
        )
        .with_children(|btn| {
            btn.spawn((
                Text::new(initial_label),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(COL_TEXT_BRIGHT),
                Pickable::IGNORE,
                ToggleButtonLabel(kind),
            ));
        });
}

fn spawn_std_btn(parent: &mut ChildSpawnerCommands, label: &str, action: UiAction) -> Entity {
    spawn_pill_button(
        parent,
        label,
        action,
        Color::srgba(0.11, 0.16, 0.22, 0.96),
        Color::srgba(0.18, 0.28, 0.36, 0.98),
        Color::srgba(0.26, 0.36, 0.46, 0.85),
    )
}

fn spawn_accent_btn(parent: &mut ChildSpawnerCommands, label: &str, action: UiAction) -> Entity {
    spawn_pill_button(
        parent,
        label,
        action,
        Color::srgba(0.09, 0.30, 0.32, 0.96),
        Color::srgba(0.14, 0.45, 0.47, 0.98),
        Color::srgba(0.24, 0.86, 0.80, 0.90),
    )
}

fn spawn_danger_btn(parent: &mut ChildSpawnerCommands, label: &str, action: UiAction) -> Entity {
    spawn_pill_button(
        parent,
        label,
        action,
        Color::srgba(0.32, 0.11, 0.13, 0.96),
        Color::srgba(0.48, 0.17, 0.19, 0.98),
        Color::srgba(0.88, 0.30, 0.32, 0.85),
    )
}

/// Big graphical Main Menu action tile with a bright colored left accent bar & subtle subtitle.
fn spawn_menu_action_tile(
    parent: &mut ChildSpawnerCommands,
    title: &str,
    subtitle: &str,
    accent: Color,
    action: UiAction,
) {
    let base_bg = Color::srgba(0.08, 0.12, 0.18, 0.96);
    let hover_bg = Color::srgba(0.13, 0.20, 0.28, 0.98);
    parent
        .spawn((
            Node {
                width: Val::Percent(100.0),
                padding: UiRect::axes(Val::Px(14.0), Val::Px(11.0)),
                border: UiRect {
                    left: Val::Px(4.0),
                    top: Val::Px(1.0),
                    right: Val::Px(1.0),
                    bottom: Val::Px(1.0),
                },
                border_radius: BorderRadius::all(Val::Px(7.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(base_bg),
            BorderColor {
                left: accent,
                top: COL_BORDER_DIM,
                right: COL_BORDER_DIM,
                bottom: COL_BORDER_DIM,
            },
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
                    *bg = BackgroundColor(hover_bg);
                }
            },
        )
        .observe(
            move |out: On<PointerOut>, mut bg_q: Query<&mut BackgroundColor>| {
                if let Ok(mut bg) = bg_q.get_mut(out.entity) {
                    *bg = BackgroundColor(base_bg);
                }
            },
        )
        .with_children(|tile| {
            tile.spawn((
                Text::new(title),
                TextFont {
                    font_size: FontSize::Px(15.0),
                    ..default()
                },
                TextColor(COL_TEXT_BRIGHT),
            ));
            tile.spawn((
                Text::new(subtitle),
                TextFont {
                    font_size: FontSize::Px(11.5),
                    ..default()
                },
                TextColor(COL_TEXT_MUTED),
            ));
        });
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
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(COL_TEXT_BRIGHT),
                Node {
                    width: Val::Px(148.0),
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
                spawn_std_btn(ctrl, "-", UiAction::SliderStep(kind, -1.0));

                // Smooth continuous slider track (updates proportionally on EVERY single click!)
                ctrl.spawn((
                    Node {
                        width: Val::Px(230.0),
                        height: Val::Px(18.0),
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::all(Val::Px(4.0)),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.11, 0.16, 0.22, 0.96)),
                    BorderColor::all(COL_BORDER_DIM),
                ))
                .with_children(|track| {
                    // Continuous cyan fill bar driven directly by exact normalized fraction
                    track.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(0.0),
                            top: Val::Px(0.0),
                            width: Val::Percent(50.0),
                            height: Val::Percent(100.0),
                            border_radius: BorderRadius::all(Val::Px(3.0)),
                            ..default()
                        },
                        BackgroundColor(COL_ACCENT_CYAN),
                        Pickable::IGNORE,
                        SliderFillBar(kind),
                    ));
                    // 20 fine transparent click zones overlaying the track so clicking anywhere sets exact value
                    track
                        .spawn(Node {
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            flex_direction: FlexDirection::Row,
                            ..default()
                        })
                        .with_children(|hits| {
                            for idx in 0..SLIDER_HIT_STEPS {
                                let frac = idx as f32 / (SLIDER_HIT_STEPS - 1) as f32;
                                hits.spawn(Node {
                                    flex_grow: 1.0,
                                    height: Val::Percent(100.0),
                                    ..default()
                                })
                                .observe(
                                    move |_click: On<PointerClick>,
                                          mut writer: MessageWriter<TriggerUiAction>| {
                                        writer.write(TriggerUiAction(UiAction::SliderSetFrac(
                                            kind, frac,
                                        )));
                                    },
                                );
                            }
                        });
                });

                spawn_std_btn(ctrl, "+", UiAction::SliderStep(kind, 1.0));
            });

            row.spawn((
                Text::new("--"),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(COL_ACCENT_CYAN),
                Node {
                    width: Val::Px(64.0),
                    justify_content: JustifyContent::FlexEnd,
                    ..default()
                },
                SliderValueText(kind),
            ));
        });
}

fn spawn_audio_dropdown_row(
    panel: &mut ChildSpawnerCommands,
    title: &str,
    kind: AudioDropdownKind,
) {
    let base_bg = Color::srgba(0.08, 0.12, 0.18, 0.96);
    let hover_bg = Color::srgba(0.13, 0.21, 0.29, 0.98);

    panel
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(5.0),
            width: Val::Percent(100.0),
            margin: UiRect::top(Val::Px(3.0)),
            ..default()
        })
        .with_children(|wrap| {
            // Header row: label + clickable dropdown trigger pill
            wrap.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                width: Val::Percent(100.0),
                ..default()
            })
            .with_children(|row| {
                row.spawn((
                    Text::new(title),
                    TextFont {
                        font_size: FontSize::Px(13.0),
                        ..default()
                    },
                    TextColor(COL_TEXT_BRIGHT),
                ));

                row.spawn((
                    Node {
                        width: Val::Px(360.0),
                        padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                        border: UiRect::all(Val::Px(1.5)),
                        border_radius: BorderRadius::all(Val::Px(6.0)),
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(8.0),
                        ..default()
                    },
                    BackgroundColor(base_bg),
                    BorderColor::all(COL_BORDER_DIM),
                ))
                .observe(
                    move |_click: On<PointerClick>, mut writer: MessageWriter<TriggerUiAction>| {
                        writer.write(TriggerUiAction(UiAction::ToggleAudioDropdown(kind)));
                    },
                )
                .observe(
                    move |over: On<PointerOver>, mut bg_q: Query<&mut BackgroundColor>| {
                        if let Ok(mut bg) = bg_q.get_mut(over.entity) {
                            *bg = BackgroundColor(hover_bg);
                        }
                    },
                )
                .observe(
                    move |out: On<PointerOut>, mut bg_q: Query<&mut BackgroundColor>| {
                        if let Ok(mut bg) = bg_q.get_mut(out.entity) {
                            *bg = BackgroundColor(base_bg);
                        }
                    },
                )
                .with_children(|btn| {
                    let lbl = btn
                        .spawn((
                            Text::new("System Default"),
                            TextFont {
                                font_size: FontSize::Px(12.5),
                                ..default()
                            },
                            TextColor(COL_TEXT_BRIGHT),
                            Pickable::IGNORE,
                        ))
                        .id();
                    match kind {
                        AudioDropdownKind::Output => {
                            btn.commands().entity(lbl).insert(OutputDeviceLabel);
                        }
                        AudioDropdownKind::Input => {
                            btn.commands().entity(lbl).insert(InputDeviceLabel);
                        }
                    }

                    btn.spawn((
                        Text::new("▼"),
                        TextFont {
                            font_size: FontSize::Px(11.0),
                            ..default()
                        },
                        TextColor(COL_ACCENT_CYAN),
                        Pickable::IGNORE,
                        AudioDropdownChevron(kind),
                    ));
                });
            });

            // Expandable clickable device list
            wrap.spawn((
                Node {
                    width: Val::Percent(100.0),
                    padding: UiRect::all(Val::Px(6.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(6.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(3.0),
                    display: Display::None,
                    ..default()
                },
                BackgroundColor(Color::srgba(0.035, 0.055, 0.085, 0.98)),
                BorderColor::all(COL_ACCENT_CYAN),
                AudioDropdownList(kind),
            ))
            .with_children(|list| {
                for idx in 0..AUDIO_DROPDOWN_ROWS {
                    list.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
                            border: UiRect::all(Val::Px(1.0)),
                            border_radius: BorderRadius::all(Val::Px(4.0)),
                            display: Display::None,
                            ..default()
                        },
                        BackgroundColor(COL_CARD_BG),
                        BorderColor::all(COL_BORDER_DIM),
                        AudioDropdownRow {
                            kind,
                            idx,
                            hovered: false,
                        },
                    ))
                    .observe(
                        move |_click: On<PointerClick>,
                              mut writer: MessageWriter<TriggerUiAction>| {
                            writer.write(TriggerUiAction(UiAction::SelectAudioRow(kind, idx)));
                        },
                    )
                    .observe(
                        |over: On<PointerOver>, mut row_q: Query<&mut AudioDropdownRow>| {
                            if let Ok(mut r) = row_q.get_mut(over.entity) {
                                r.hovered = true;
                            }
                        },
                    )
                    .observe(
                        |out: On<PointerOut>, mut row_q: Query<&mut AudioDropdownRow>| {
                            if let Ok(mut r) = row_q.get_mut(out.entity) {
                                r.hovered = false;
                            }
                        },
                    )
                    .with_children(|r| {
                        r.spawn((
                            Text::new(""),
                            TextFont {
                                font_size: FontSize::Px(12.0),
                                ..default()
                            },
                            TextColor(COL_TEXT_BRIGHT),
                            Pickable::IGNORE,
                            AudioDropdownRowText { kind, idx },
                        ));
                    });
                }

                list.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Center,
                        margin: UiRect::top(Val::Px(2.0)),
                        display: Display::None,
                        ..default()
                    },
                    AudioDropdownPagerRow(kind),
                ))
                .with_children(|pager| {
                    spawn_std_btn(pager, "Prev", UiAction::AudioListPage(-1));
                    pager.spawn((
                        Text::new("Page 1 / 1"),
                        TextFont {
                            font_size: FontSize::Px(11.0),
                            ..default()
                        },
                        TextColor(COL_TEXT_MUTED),
                        AudioDropdownPageLabel(kind),
                    ));
                    spawn_std_btn(pager, "Next", UiAction::AudioListPage(1));
                });
            });
        });
}

fn shop_item_visual_accent(item: ShopCatalogItem) -> (Color, &'static str) {
    match item {
        ShopCatalogItem::BatteryCell => (Color::srgb(0.20, 0.92, 0.96), "BAT"),
        ShopCatalogItem::FlarePack => (Color::srgb(1.0, 0.42, 0.18), "FLR"),
        ShopCatalogItem::FoamCanister => (Color::srgb(0.32, 0.88, 0.72), "FOM"),
        ShopCatalogItem::NoiseDecoys => (Color::srgb(1.0, 0.78, 0.22), "DCY"),
        ShopCatalogItem::AidKit => (Color::srgb(0.98, 0.30, 0.34), "AID"),
        ShopCatalogItem::ToolBeltUpgrade => (Color::srgb(0.55, 0.72, 1.0), "BLT"),
        ShopCatalogItem::BatteryPouchUpgrade => (Color::srgb(0.32, 0.84, 1.0), "PCH"),
        ShopCatalogItem::Headlamp => (Color::srgb(1.0, 0.92, 0.48), "HLP"),
        ShopCatalogItem::HeadlampCellMk2 => (Color::srgb(1.0, 0.80, 0.24), "HL2"),
        ShopCatalogItem::StealthBoots => (Color::srgb(0.56, 0.42, 0.98), "BTS"),
    }
}

// =============================================================================
// SETUP HUD (GRAPHICAL OVERHAUL)
// =============================================================================

fn setup_hud(mut commands: Commands) {
    // =========================================================================
    // 1. MAIN MENU DASHBOARD (2-Column Graphical Subterranean Card)
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
            BackgroundColor(Color::srgba(0.015, 0.025, 0.04, 0.76)),
            GlobalZIndex(20),
            Visibility::Visible,
            UiPanelKind::MainMenu,
        ))
        .with_children(|backdrop| {
            backdrop
                .spawn((
                    Node {
                        width: Val::Px(760.0),
                        padding: UiRect::all(Val::Px(24.0)),
                        border: UiRect {
                            top: Val::Px(3.0),
                            left: Val::Px(1.5),
                            right: Val::Px(1.5),
                            bottom: Val::Px(1.5),
                        },
                        border_radius: BorderRadius::all(Val::Px(12.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(16.0),
                        ..default()
                    },
                    BackgroundColor(COL_PANEL_BG),
                    BorderColor {
                        top: COL_ACCENT_CYAN,
                        left: COL_BORDER_DIM,
                        right: COL_BORDER_DIM,
                        bottom: COL_BORDER_DIM,
                    },
                ))
                .with_children(|card| {
                    // Header Banner
                    card.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Center,
                        width: Val::Percent(100.0),
                        ..default()
                    })
                    .with_children(|hdr| {
                        hdr.spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(3.0),
                            ..default()
                        })
                        .with_children(|left| {
                            left.spawn((
                                Text::new("ECHOES BELOW"),
                                TextFont {
                                    font_size: FontSize::Px(34.0),
                                    ..default()
                                },
                                TextColor(COL_ACCENT_CYAN),
                            ));
                            left.spawn((
                                Node {
                                    width: Val::Px(130.0),
                                    height: Val::Px(3.0),
                                    border_radius: BorderRadius::all(Val::Px(2.0)),
                                    ..default()
                                },
                                BackgroundColor(Color::srgba(0.22, 0.92, 0.86, 0.65)),
                            ));
                        });

                        hdr.spawn((
                            Node {
                                padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
                                border: UiRect::all(Val::Px(1.0)),
                                border_radius: BorderRadius::all(Val::Px(12.0)),
                                ..default()
                            },
                            BackgroundColor(Color::srgba(0.08, 0.16, 0.22, 0.92)),
                            BorderColor::all(Color::srgba(0.22, 0.92, 0.86, 0.45)),
                        ))
                        .with_children(|pill| {
                            pill.spawn((
                                Text::new(format!("P2P • ACOUSTIC CO-OP • {APP_VERSION}")),
                                TextFont {
                                    font_size: FontSize::Px(11.5),
                                    ..default()
                                },
                                TextColor(COL_ACCENT_CYAN),
                            ));
                        });
                    });

                    // 2-Column Body
                    card.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(18.0),
                        width: Val::Percent(100.0),
                        ..default()
                    })
                    .with_children(|cols| {
                        // LEFT COLUMN: Miner Profile, Visual Helmet Swatches, Save Slot Card
                        cols.spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(12.0),
                            width: Val::Px(410.0),
                            ..default()
                        })
                        .with_children(|left| {
                            // Miner Name Box
                            left.spawn((
                                Node {
                                    padding: UiRect::all(Val::Px(12.0)),
                                    border: UiRect::all(Val::Px(1.0)),
                                    border_radius: BorderRadius::all(Val::Px(8.0)),
                                    flex_direction: FlexDirection::Column,
                                    row_gap: Val::Px(6.0),
                                    ..default()
                                },
                                BackgroundColor(COL_CARD_BG),
                                BorderColor::all(COL_BORDER_DIM),
                            ))
                            .with_children(|sec| {
                                sec.spawn((
                                    Text::new("MINER CALLSIGN"),
                                    TextFont {
                                        font_size: FontSize::Px(11.0),
                                        ..default()
                                    },
                                    TextColor(COL_TEXT_MUTED),
                                ));
                                sec.spawn((
                                    Node {
                                        width: Val::Percent(100.0),
                                        padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                                        border: UiRect::all(Val::Px(1.5)),
                                        border_radius: BorderRadius::all(Val::Px(6.0)),
                                        ..default()
                                    },
                                    BackgroundColor(Color::srgba(0.05, 0.08, 0.12, 0.98)),
                                    BorderColor::all(COL_BORDER_DIM),
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
                                        TextColor(COL_TEXT_BRIGHT),
                                        MainMenuNameText,
                                    ));
                                });

                                // Visual Helmet Visor Picker (6 mini-helmet tiles!)
                                sec.spawn((
                                    Text::new("HELMET VISOR"),
                                    TextFont {
                                        font_size: FontSize::Px(11.0),
                                        ..default()
                                    },
                                    TextColor(COL_TEXT_MUTED),
                                    Node {
                                        margin: UiRect::top(Val::Px(4.0)),
                                        ..default()
                                    },
                                ));
                                sec.spawn(Node {
                                    flex_direction: FlexDirection::Row,
                                    column_gap: Val::Px(8.0),
                                    ..default()
                                })
                                .with_children(|row| {
                                    for (idx, (_name, rgb)) in VISOR_PALETTE.iter().enumerate() {
                                        let visor_col = Color::srgb(rgb[0], rgb[1], rgb[2]);
                                        row.spawn((
                                            Node {
                                                width: Val::Px(54.0),
                                                height: Val::Px(44.0),
                                                border: UiRect::all(Val::Px(2.0)),
                                                border_radius: BorderRadius::all(Val::Px(8.0)),
                                                justify_content: JustifyContent::Center,
                                                align_items: AlignItems::Center,
                                                ..default()
                                            },
                                            BackgroundColor(Color::srgba(0.11, 0.14, 0.19, 0.96)),
                                            BorderColor::all(COL_BORDER_DIM),
                                            VisorHelmetButton(idx),
                                        ))
                                        .observe(
                                            move |_click: On<PointerClick>,
                                                  mut writer: MessageWriter<TriggerUiAction>| {
                                                writer.write(TriggerUiAction(
                                                    UiAction::SelectVisor(idx),
                                                ));
                                            },
                                        )
                                        .with_children(|tile| {
                                            // Mini helmet shell graphic with glowing visor bar
                                            tile.spawn((
                                                Node {
                                                    width: Val::Px(32.0),
                                                    height: Val::Px(26.0),
                                                    border_radius: BorderRadius::all(Val::Px(7.0)),
                                                    justify_content: JustifyContent::Center,
                                                    align_items: AlignItems::Center,
                                                    ..default()
                                                },
                                                BackgroundColor(Color::srgb(0.18, 0.21, 0.26)),
                                            ))
                                            .with_children(|helm| {
                                                helm.spawn((
                                                    Node {
                                                        width: Val::Px(24.0),
                                                        height: Val::Px(9.0),
                                                        border_radius: BorderRadius::all(
                                                            Val::Px(3.0),
                                                        ),
                                                        ..default()
                                                    },
                                                    BackgroundColor(visor_col),
                                                ));
                                            });
                                        });
                                    }
                                });
                            });

                            // Graphical Campaign Save Card
                            left.spawn((
                                Node {
                                    padding: UiRect::all(Val::Px(12.0)),
                                    border: UiRect::all(Val::Px(1.0)),
                                    border_radius: BorderRadius::all(Val::Px(8.0)),
                                    flex_direction: FlexDirection::Column,
                                    row_gap: Val::Px(8.0),
                                    ..default()
                                },
                                BackgroundColor(COL_CARD_BG),
                                BorderColor::all(COL_BORDER_DIM),
                            ))
                            .with_children(|sec| {
                                sec.spawn(Node {
                                    flex_direction: FlexDirection::Row,
                                    justify_content: JustifyContent::SpaceBetween,
                                    align_items: AlignItems::Center,
                                    width: Val::Percent(100.0),
                                    ..default()
                                })
                                .with_children(|top| {
                                    top.spawn((
                                        Text::new("EXPEDITION SAVE"),
                                        TextFont {
                                            font_size: FontSize::Px(11.0),
                                            ..default()
                                        },
                                        TextColor(COL_TEXT_MUTED),
                                    ));
                                    // 3 Tier Unlock Pips
                                    top.spawn(Node {
                                        flex_direction: FlexDirection::Row,
                                        column_gap: Val::Px(4.0),
                                        ..default()
                                    })
                                    .with_children(|pips| {
                                        for tier in 1..=3u8 {
                                            pips.spawn((
                                                Node {
                                                    width: Val::Px(10.0),
                                                    height: Val::Px(10.0),
                                                    border_radius: BorderRadius::all(Val::Px(5.0)),
                                                    ..default()
                                                },
                                                BackgroundColor(COL_BORDER_DIM),
                                                SaveTierPip(tier),
                                            ));
                                        }
                                    });
                                });

                                // Clickable Save Selector Dropdown Header
                                let dd_base = Color::srgba(0.05, 0.08, 0.12, 0.96);
                                let dd_hover = Color::srgba(0.09, 0.15, 0.22, 0.98);
                                sec.spawn((
                                    Node {
                                        width: Val::Percent(100.0),
                                        padding: UiRect::axes(Val::Px(11.0), Val::Px(8.0)),
                                        border: UiRect::all(Val::Px(1.5)),
                                        border_radius: BorderRadius::all(Val::Px(6.0)),
                                        flex_direction: FlexDirection::Row,
                                        justify_content: JustifyContent::SpaceBetween,
                                        align_items: AlignItems::Center,
                                        column_gap: Val::Px(8.0),
                                        ..default()
                                    },
                                    BackgroundColor(dd_base),
                                    BorderColor::all(COL_BORDER_DIM),
                                ))
                                .observe(
                                    |_click: On<PointerClick>,
                                     mut writer: MessageWriter<TriggerUiAction>| {
                                        writer.write(TriggerUiAction(UiAction::ToggleSaveDropdown));
                                    },
                                )
                                .observe(
                                    move |over: On<PointerOver>,
                                          mut bg_q: Query<&mut BackgroundColor>| {
                                        if let Ok(mut bg) = bg_q.get_mut(over.entity) {
                                            *bg = BackgroundColor(dd_hover);
                                        }
                                    },
                                )
                                .observe(
                                    move |out: On<PointerOut>,
                                          mut bg_q: Query<&mut BackgroundColor>| {
                                        if let Ok(mut bg) = bg_q.get_mut(out.entity) {
                                            *bg = BackgroundColor(dd_base);
                                        }
                                    },
                                )
                                .with_children(|slot_box| {
                                    slot_box
                                        .spawn((
                                            Node {
                                                flex_direction: FlexDirection::Column,
                                                row_gap: Val::Px(2.0),
                                                ..default()
                                            },
                                            Pickable::IGNORE,
                                        ))
                                        .with_children(|col| {
                                            col.spawn((
                                                Text::new("default"),
                                                TextFont {
                                                    font_size: FontSize::Px(14.0),
                                                    ..default()
                                                },
                                                TextColor(COL_ACCENT_CYAN),
                                                Pickable::IGNORE,
                                                MainMenuSaveNameText,
                                            ));
                                            col.spawn((
                                                Text::new("New Campaign • 100 cr"),
                                                TextFont {
                                                    font_size: FontSize::Px(11.5),
                                                    ..default()
                                                },
                                                TextColor(COL_TEXT_MUTED),
                                                Pickable::IGNORE,
                                                MainMenuSaveMetaText,
                                            ));
                                        });

                                    slot_box.spawn((
                                        Text::new("▼"),
                                        TextFont {
                                            font_size: FontSize::Px(12.0),
                                            ..default()
                                        },
                                        TextColor(COL_ACCENT_CYAN),
                                        Pickable::IGNORE,
                                        SaveDropdownChevron,
                                    ));
                                });

                                // Expandable Clickable Save Slot List (up to 6 visible rows + optional pager)
                                sec.spawn((
                                    Node {
                                        width: Val::Percent(100.0),
                                        padding: UiRect::all(Val::Px(6.0)),
                                        border: UiRect::all(Val::Px(1.0)),
                                        border_radius: BorderRadius::all(Val::Px(6.0)),
                                        flex_direction: FlexDirection::Column,
                                        row_gap: Val::Px(4.0),
                                        display: Display::None,
                                        ..default()
                                    },
                                    BackgroundColor(Color::srgba(0.035, 0.055, 0.085, 0.98)),
                                    BorderColor::all(COL_ACCENT_CYAN),
                                    SaveDropdownList,
                                ))
                                .with_children(|list| {
                                    for row_idx in 0..SAVE_DROPDOWN_ROWS {
                                        list.spawn((
                                            Node {
                                                width: Val::Percent(100.0),
                                                padding: UiRect::axes(Val::Px(9.0), Val::Px(6.0)),
                                                border: UiRect::all(Val::Px(1.0)),
                                                border_radius: BorderRadius::all(Val::Px(5.0)),
                                                flex_direction: FlexDirection::Row,
                                                justify_content: JustifyContent::SpaceBetween,
                                                align_items: AlignItems::Center,
                                                column_gap: Val::Px(8.0),
                                                display: Display::None,
                                                ..default()
                                            },
                                            BackgroundColor(COL_CARD_BG),
                                            BorderColor::all(COL_BORDER_DIM),
                                            SaveDropdownRow {
                                                idx: row_idx,
                                                hovered: false,
                                            },
                                        ))
                                        .observe(
                                            move |_click: On<PointerClick>,
                                                  mut writer: MessageWriter<TriggerUiAction>| {
                                                writer.write(TriggerUiAction(
                                                    UiAction::SelectSaveRow(row_idx),
                                                ));
                                            },
                                        )
                                        .observe(
                                            |over: On<PointerOver>,
                                             mut row_q: Query<&mut SaveDropdownRow>| {
                                                if let Ok(mut r) = row_q.get_mut(over.entity) {
                                                    r.hovered = true;
                                                }
                                            },
                                        )
                                        .observe(
                                            |out: On<PointerOut>,
                                             mut row_q: Query<&mut SaveDropdownRow>| {
                                                if let Ok(mut r) = row_q.get_mut(out.entity) {
                                                    r.hovered = false;
                                                }
                                            },
                                        )
                                        .with_children(|r| {
                                            r.spawn((
                                                Text::new(""),
                                                TextFont {
                                                    font_size: FontSize::Px(13.0),
                                                    ..default()
                                                },
                                                TextColor(COL_TEXT_BRIGHT),
                                                Pickable::IGNORE,
                                                SaveDropdownRowName(row_idx),
                                            ));
                                            r.spawn((
                                                Text::new(""),
                                                TextFont {
                                                    font_size: FontSize::Px(11.0),
                                                    ..default()
                                                },
                                                TextColor(COL_TEXT_MUTED),
                                                Pickable::IGNORE,
                                                SaveDropdownRowMeta(row_idx),
                                            ));
                                        });
                                    }

                                    list.spawn((
                                        Node {
                                            width: Val::Percent(100.0),
                                            flex_direction: FlexDirection::Row,
                                            justify_content: JustifyContent::SpaceBetween,
                                            align_items: AlignItems::Center,
                                            margin: UiRect::top(Val::Px(2.0)),
                                            display: Display::None,
                                            ..default()
                                        },
                                        SaveDropdownPagerRow,
                                    ))
                                    .with_children(|pager| {
                                        spawn_std_btn(pager, "Prev", UiAction::SaveListPage(-1));
                                        pager.spawn((
                                            Text::new("1 / 1"),
                                            TextFont {
                                                font_size: FontSize::Px(11.0),
                                                ..default()
                                            },
                                            TextColor(COL_TEXT_MUTED),
                                            SaveDropdownPageLabel,
                                        ));
                                        spawn_std_btn(pager, "Next", UiAction::SaveListPage(1));
                                    });
                                });

                                sec.spawn(Node {
                                    flex_direction: FlexDirection::Row,
                                    column_gap: Val::Px(6.0),
                                    ..default()
                                })
                                .with_children(|row| {
                                    spawn_std_btn(row, "+ New Save", UiAction::OpenNewSavePrompt);
                                    spawn_std_btn(row, "Rename", UiAction::OpenRenameSavePrompt);
                                    spawn_std_btn(row, "Refresh", UiAction::RefreshSaveSlots);
                                });
                            });
                        });

                        // RIGHT COLUMN: Big Visual Action Tiles
                        cols.spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(9.0),
                            flex_grow: 1.0,
                            ..default()
                        })
                        .with_children(|right| {
                            spawn_menu_action_tile(
                                right,
                                "PLAY SOLO",
                                "Load selected save offline",
                                COL_ACCENT_CYAN,
                                UiAction::PlaySolo,
                            );
                            spawn_menu_action_tile(
                                right,
                                "HOST CO-OP",
                                "Generate ECHO-... P2P code",
                                COL_ACCENT_GREEN,
                                UiAction::HostLobby,
                            );
                            spawn_menu_action_tile(
                                right,
                                "JOIN BY CODE",
                                "Paste friend's clipboard code",
                                COL_ACCENT_AMBER,
                                UiAction::PasteAndJoin,
                            );
                            right
                                .spawn(Node {
                                    flex_direction: FlexDirection::Row,
                                    column_gap: Val::Px(6.0),
                                    margin: UiRect::top(Val::Px(4.0)),
                                    ..default()
                                })
                                .with_children(|row| {
                                    spawn_std_btn(row, "Settings", UiAction::OpenSettings);
                                    spawn_std_btn(row, "Cancel", UiAction::CancelConnecting);
                                    spawn_danger_btn(row, "Quit", UiAction::QuitGame);
                                });
                        });
                    });

                    // Bottom Status Pill
                    card.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            padding: UiRect::axes(Val::Px(12.0), Val::Px(7.0)),
                            border_radius: BorderRadius::all(Val::Px(6.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.07, 0.09, 0.13, 0.92)),
                    ))
                    .with_children(|bar| {
                        bar.spawn((
                            Text::new("Ready — Select a save slot and dive in"),
                            TextFont {
                                font_size: FontSize::Px(12.5),
                                ..default()
                            },
                            TextColor(COL_ACCENT_AMBER),
                            MainMenuStatusText,
                        ));
                    });
                });
        });

    // =========================================================================
    // 2. GRAPHICAL SETTINGS PANEL (Video + Audio + 10-Bar LED Mic Meter)
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
            BackgroundColor(Color::srgba(0.015, 0.025, 0.04, 0.80)),
            GlobalZIndex(50),
            Visibility::Hidden,
            UiPanelKind::Settings,
        ))
        .with_children(|backdrop| {
            backdrop
                .spawn((
                    Node {
                        width: Val::Px(640.0),
                        padding: UiRect::all(Val::Px(22.0)),
                        border: UiRect {
                            top: Val::Px(3.0),
                            left: Val::Px(1.5),
                            right: Val::Px(1.5),
                            bottom: Val::Px(1.5),
                        },
                        border_radius: BorderRadius::all(Val::Px(12.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(10.0),
                        ..default()
                    },
                    BackgroundColor(COL_PANEL_BG),
                    BorderColor {
                        top: COL_ACCENT_CYAN,
                        left: COL_BORDER_DIM,
                        right: COL_BORDER_DIM,
                        bottom: COL_BORDER_DIM,
                    },
                ))
                .with_children(|panel| {
                    panel.spawn((
                        Text::new("SETTINGS"),
                        TextFont {
                            font_size: FontSize::Px(20.0),
                            ..default()
                        },
                        TextColor(COL_ACCENT_CYAN),
                    ));

                    spawn_slider_row(panel, "Field of View", SliderKind::Fov);

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
                                    font_size: FontSize::Px(13.0),
                                    ..default()
                                },
                                TextColor(COL_TEXT_BRIGHT),
                            ));
                            spawn_toggle_btn(
                                row,
                                "Windowed",
                                UiAction::ToggleFullscreen,
                                ToggleKind::Fullscreen,
                            );
                        });

                    // Audio Output clickable dropdown selector
                    spawn_audio_dropdown_row(panel, "Output Device", AudioDropdownKind::Output);

                    spawn_slider_row(panel, "Master Volume", SliderKind::MasterVolume);
                    spawn_slider_row(panel, "Voice Volume", SliderKind::VoiceVolume);

                    // Audio Input clickable dropdown selector
                    spawn_audio_dropdown_row(panel, "Microphone", AudioDropdownKind::Input);

                    spawn_slider_row(panel, "Mic Gain", SliderKind::MicGain);
                    spawn_slider_row(panel, "Noise Gate", SliderKind::MicGate);

                    // Graphical 10-Bar LED Mic Meter inside Settings
                    panel
                        .spawn((
                            Node {
                                flex_direction: FlexDirection::Row,
                                align_items: AlignItems::Center,
                                justify_content: JustifyContent::SpaceBetween,
                                padding: UiRect::axes(Val::Px(12.0), Val::Px(8.0)),
                                border_radius: BorderRadius::all(Val::Px(7.0)),
                                width: Val::Percent(100.0),
                                ..default()
                            },
                            BackgroundColor(COL_CARD_BG),
                        ))
                        .with_children(|bar| {
                            bar.spawn(Node {
                                flex_direction: FlexDirection::Row,
                                align_items: AlignItems::FlexEnd,
                                column_gap: Val::Px(4.0),
                                height: Val::Px(22.0),
                                ..default()
                            })
                            .with_children(|leds| {
                                for i in 0..MIC_LED_BARS {
                                    let h = 8.0 + (i as f32 * 1.4);
                                    leds.spawn((
                                        Node {
                                            width: Val::Px(12.0),
                                            height: Val::Px(h),
                                            border_radius: BorderRadius::all(Val::Px(2.0)),
                                            ..default()
                                        },
                                        BackgroundColor(Color::srgba(0.14, 0.19, 0.24, 0.9)),
                                        SettingsMicLed(i),
                                    ));
                                }
                            });
                            bar.spawn((
                                Text::new("SILENT"),
                                TextFont {
                                    font_size: FontSize::Px(12.5),
                                    ..default()
                                },
                                TextColor(COL_ACCENT_CYAN),
                                SettingsMicBadgeText,
                            ));
                        });

                    // Auto-Updater row inside Settings (Pre-release toggle + Check for Updates)
                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::SpaceBetween,
                            width: Val::Percent(100.0),
                            column_gap: Val::Px(8.0),
                            margin: UiRect::top(Val::Px(2.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn(Node {
                                flex_direction: FlexDirection::Row,
                                align_items: AlignItems::Center,
                                column_gap: Val::Px(6.0),
                                ..default()
                            })
                            .with_children(|left| {
                                spawn_toggle_btn(
                                    left,
                                    "Pre-releases: ON",
                                    UiAction::ToggleIncludePrereleases,
                                    ToggleKind::IncludePrereleases,
                                );
                                spawn_std_btn(
                                    left,
                                    "Check for Updates",
                                    UiAction::CheckForUpdates,
                                );
                            });
                            row.spawn((
                                Text::new(format!("Installed: {APP_VERSION}")),
                                TextFont {
                                    font_size: FontSize::Px(12.0),
                                    ..default()
                                },
                                TextColor(COL_TEXT_MUTED),
                                SettingsUpdaterStatusText,
                            ));
                        });

                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            justify_content: JustifyContent::SpaceBetween,
                            width: Val::Percent(100.0),
                            margin: UiRect::top(Val::Px(4.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn(Node {
                                flex_direction: FlexDirection::Row,
                                column_gap: Val::Px(6.0),
                                ..default()
                            })
                            .with_children(|left| {
                                spawn_std_btn(left, "Refresh Devices", UiAction::RefreshAudioDevices);
                                spawn_toggle_btn(
                                    left,
                                    "Mic: LIVE",
                                    UiAction::ToggleMute,
                                    ToggleKind::Mute,
                                );
                                spawn_toggle_btn(
                                    left,
                                    "Self-Echo: OFF",
                                    UiAction::ToggleSelfEcho,
                                    ToggleKind::SelfEcho,
                                );
                            });
                            spawn_accent_btn(row, "Done", UiAction::CloseSettings);
                        });
                });
        });

    // =========================================================================
    // 3. IN-GAME GRAPHICAL HUD (Diegetic Pods, Bars, Morphing Reticle, Hotbar)
    // =========================================================================
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            GlobalZIndex(10),
            Pickable::IGNORE,
            Visibility::Hidden,
            UiPanelKind::InGameHud,
        ))
        .with_children(|hud| {
            // Damage / Downed subtle red screen vignette (must ignore pointer hits!)
            hud.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.85, 0.08, 0.08, 0.0)),
                Pickable::IGNORE,
                DamageFlashOverlay,
            ));

            // -----------------------------------------------------------------
            // TOP-LEFT DIVE TELEMETRY POD (Tier + Credits + Elevator Bar + HP Bar)
            // -----------------------------------------------------------------
            hud.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(16.0),
                    left: Val::Px(16.0),
                    width: Val::Px(246.0),
                    padding: UiRect::all(Val::Px(11.0)),
                    border: UiRect {
                        left: Val::Px(3.0),
                        top: Val::Px(1.0),
                        right: Val::Px(1.0),
                        bottom: Val::Px(1.0),
                    },
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(8.0),
                    ..default()
                },
                BackgroundColor(COL_HUD_POD_BG),
                BorderColor {
                    left: COL_ACCENT_CYAN,
                    top: COL_BORDER_DIM,
                    right: COL_BORDER_DIM,
                    bottom: COL_BORDER_DIM,
                },
            ))
            .with_children(|pod| {
                // Row 1: Tier Pill + Credits Pill + Elevator Phase Tag
                pod.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::SpaceBetween,
                    width: Val::Percent(100.0),
                    ..default()
                })
                .with_children(|row| {
                    row.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(6.0),
                        ..default()
                    })
                    .with_children(|badges| {
                        badges
                            .spawn((
                                Node {
                                    padding: UiRect::axes(Val::Px(7.0), Val::Px(2.0)),
                                    border_radius: BorderRadius::all(Val::Px(4.0)),
                                    ..default()
                                },
                                BackgroundColor(Color::srgba(0.11, 0.34, 0.36, 0.95)),
                            ))
                            .with_children(|b| {
                                b.spawn((
                                    Text::new("T1"),
                                    TextFont {
                                        font_size: FontSize::Px(12.0),
                                        ..default()
                                    },
                                    TextColor(COL_ACCENT_CYAN),
                                    HudTierBadgeText,
                                ));
                            });

                        badges.spawn((
                            Text::new("100 cr"),
                            TextFont {
                                font_size: FontSize::Px(13.5),
                                ..default()
                            },
                            TextColor(COL_ACCENT_AMBER),
                            HudCreditsText,
                        ));
                    });

                    row.spawn((
                        Text::new("SHED"),
                        TextFont {
                            font_size: FontSize::Px(11.5),
                            ..default()
                        },
                        TextColor(COL_TEXT_MUTED),
                        HudPhaseText,
                    ));
                });

                // Elevator / Winch Graphical Progress Bar
                pod.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(5.0),
                        border_radius: BorderRadius::all(Val::Px(3.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.12, 0.17, 0.22, 0.95)),
                ))
                .with_children(|track| {
                    track.spawn((
                        Node {
                            width: Val::Percent(0.0),
                            height: Val::Percent(100.0),
                            border_radius: BorderRadius::all(Val::Px(3.0)),
                            ..default()
                        },
                        BackgroundColor(COL_ACCENT_AMBER),
                        HudElevatorProgressFill,
                    ));
                });

                // Health Bar Row (Heart/Cross Icon + Fill Bar + Tag)
                pod.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(8.0),
                    width: Val::Percent(100.0),
                    ..default()
                })
                .with_children(|row| {
                    // Mini visual cross icon
                    row.spawn((
                        Node {
                            width: Val::Px(14.0),
                            height: Val::Px(14.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            border_radius: BorderRadius::all(Val::Px(3.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.16, 0.42, 0.28, 0.95)),
                    ))
                    .with_children(|icon| {
                        icon.spawn((
                            Text::new("+"),
                            TextFont {
                                font_size: FontSize::Px(12.0),
                                ..default()
                            },
                            TextColor(COL_ACCENT_GREEN),
                        ));
                    });

                    row.spawn((
                        Node {
                            flex_grow: 1.0,
                            height: Val::Px(9.0),
                            border_radius: BorderRadius::all(Val::Px(4.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.12, 0.16, 0.20, 0.95)),
                    ))
                    .with_children(|track| {
                        track.spawn((
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Percent(100.0),
                                border_radius: BorderRadius::all(Val::Px(4.0)),
                                ..default()
                            },
                            BackgroundColor(COL_ACCENT_GREEN),
                            HudHealthFill,
                        ));
                    });

                    row.spawn((
                        Text::new("OK"),
                        TextFont {
                            font_size: FontSize::Px(11.0),
                            ..default()
                        },
                        TextColor(COL_ACCENT_GREEN),
                        HudHealthBadgeText,
                    ));
                });
            });

            // -----------------------------------------------------------------
            // TOP-RIGHT SENSOR POD (10-Bar LED Mic Meter + Visual Battery Cell)
            // -----------------------------------------------------------------
            hud.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(16.0),
                    right: Val::Px(16.0),
                    padding: UiRect::axes(Val::Px(12.0), Val::Px(10.0)),
                    border: UiRect {
                        right: Val::Px(3.0),
                        top: Val::Px(1.0),
                        left: Val::Px(1.0),
                        bottom: Val::Px(1.0),
                    },
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(14.0),
                    ..default()
                },
                BackgroundColor(COL_HUD_POD_BG),
                BorderColor {
                    right: COL_ACCENT_CYAN,
                    top: COL_BORDER_DIM,
                    left: COL_BORDER_DIM,
                    bottom: COL_BORDER_DIM,
                },
            ))
            .with_children(|pod| {
                // 10-Bar Graphical Voice LED Meter
                pod.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(6.0),
                    ..default()
                })
                .with_children(|mic_grp| {
                    // Mic status indicator dot
                    mic_grp.spawn((
                        Node {
                            width: Val::Px(9.0),
                            height: Val::Px(9.0),
                            border_radius: BorderRadius::all(Val::Px(5.0)),
                            ..default()
                        },
                        BackgroundColor(COL_ACCENT_GREEN),
                        HudMicStateDot,
                    ));
                    mic_grp
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::FlexEnd,
                            column_gap: Val::Px(3.0),
                            height: Val::Px(22.0),
                            ..default()
                        })
                        .with_children(|leds| {
                            for i in 0..MIC_LED_BARS {
                                let h = 6.0 + (i as f32 * 1.6);
                                leds.spawn((
                                    Node {
                                        width: Val::Px(5.0),
                                        height: Val::Px(h),
                                        border_radius: BorderRadius::all(Val::Px(2.0)),
                                        ..default()
                                    },
                                    BackgroundColor(Color::srgba(0.14, 0.19, 0.25, 0.85)),
                                    HudMicLed(i),
                                ));
                            }
                        });
                });

                // Divider line
                pod.spawn((
                    Node {
                        width: Val::Px(1.0),
                        height: Val::Px(22.0),
                        ..default()
                    },
                    BackgroundColor(COL_BORDER_DIM),
                ));

                // Visual Flashlight Battery Cell Graphic
                pod.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(5.0),
                    ..default()
                })
                .with_children(|bat_grp| {
                    // Lamp ON/OFF glow dot
                    bat_grp.spawn((
                        Node {
                            width: Val::Px(10.0),
                            height: Val::Px(10.0),
                            border_radius: BorderRadius::all(Val::Px(5.0)),
                            ..default()
                        },
                        BackgroundColor(COL_ACCENT_AMBER),
                        HudBatteryLampDot,
                    ));
                    // Battery body shell + terminal cap
                    bat_grp
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            ..default()
                        })
                        .with_children(|cell| {
                            cell.spawn((
                                Node {
                                    width: Val::Px(46.0),
                                    height: Val::Px(18.0),
                                    padding: UiRect::all(Val::Px(2.0)),
                                    border: UiRect::all(Val::Px(1.5)),
                                    border_radius: BorderRadius::all(Val::Px(4.0)),
                                    ..default()
                                },
                                BackgroundColor(Color::srgba(0.06, 0.09, 0.13, 0.95)),
                                BorderColor::all(Color::srgba(0.42, 0.56, 0.66, 0.9)),
                            ))
                            .with_children(|shell| {
                                shell.spawn((
                                    Node {
                                        width: Val::Percent(100.0),
                                        height: Val::Percent(100.0),
                                        border_radius: BorderRadius::all(Val::Px(2.0)),
                                        ..default()
                                    },
                                    BackgroundColor(COL_ACCENT_CYAN),
                                    HudBatteryFill,
                                ));
                            });
                            // Positive battery terminal nub
                            cell.spawn((
                                Node {
                                    width: Val::Px(3.0),
                                    height: Val::Px(8.0),
                                    border_radius: BorderRadius::all(Val::Px(1.0)),
                                    ..default()
                                },
                                BackgroundColor(Color::srgba(0.55, 0.68, 0.78, 0.9)),
                            ));
                        });
                });
            });

            // -----------------------------------------------------------------
            // OPTIONAL [F3] ACOUSTIC DIAGNOSTICS DRAWER (Hidden by default!)
            // -----------------------------------------------------------------
            hud.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(106.0),
                    left: Val::Px(16.0),
                    padding: UiRect::all(Val::Px(10.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(6.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.03, 0.05, 0.08, 0.86)),
                BorderColor::all(COL_BORDER_DIM),
                Visibility::Hidden,
                UiPanelKind::DiagDrawer,
            ))
            .with_children(|box_n| {
                box_n.spawn((
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(11.5),
                        ..default()
                    },
                    TextColor(COL_TEXT_MUTED),
                    DiagText,
                ));
            });

            // -----------------------------------------------------------------
            // PARKOUR ROOM LIVE CLOCK & LEADERBOARD POD (Shown while in Parkour Room!)
            // -----------------------------------------------------------------
            hud.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(122.0),
                    right: Val::Px(16.0),
                    width: Val::Px(268.0),
                    padding: UiRect::all(Val::Px(12.0)),
                    border: UiRect {
                        right: Val::Px(3.0),
                        top: Val::Px(1.0),
                        left: Val::Px(1.0),
                        bottom: Val::Px(1.0),
                    },
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(6.0),
                    ..default()
                },
                BackgroundColor(COL_HUD_POD_BG),
                BorderColor {
                    right: COL_ACCENT_GREEN,
                    top: COL_BORDER_DIM,
                    left: COL_BORDER_DIM,
                    bottom: COL_BORDER_DIM,
                },
                Visibility::Hidden,
                Pickable::IGNORE,
                UiPanelKind::ParkourHud,
            ))
            .with_children(|pod| {
                pod.spawn((
                    Text::new("PARKOUR CLOCK"),
                    TextFont {
                        font_size: FontSize::Px(11.0),
                        ..default()
                    },
                    TextColor(COL_ACCENT_GREEN),
                    Pickable::IGNORE,
                ));
                pod.spawn((
                    Text::new("0.00s"),
                    TextFont {
                        font_size: FontSize::Px(24.0),
                        ..default()
                    },
                    TextColor(COL_TEXT_BRIGHT),
                    Pickable::IGNORE,
                    ParkourClockText,
                ));
                pod.spawn((
                    Text::new("Pass Start Gate to begin"),
                    TextFont {
                        font_size: FontSize::Px(11.5),
                        ..default()
                    },
                    TextColor(COL_TEXT_MUTED),
                    Pickable::IGNORE,
                    ParkourClockStatusText,
                ));

                pod.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(1.0),
                        margin: UiRect::vertical(Val::Px(2.0)),
                        ..default()
                    },
                    BackgroundColor(COL_BORDER_DIM),
                    Pickable::IGNORE,
                ));

                pod.spawn((
                    Text::new("BEST RUNS LEADERBOARD"),
                    TextFont {
                        font_size: FontSize::Px(10.5),
                        ..default()
                    },
                    TextColor(COL_ACCENT_AMBER),
                    Pickable::IGNORE,
                ));

                for idx in 0..MAX_PARKOUR_LEADERBOARD {
                    pod.spawn((
                        Text::new(""),
                        TextFont {
                            font_size: FontSize::Px(12.0),
                            ..default()
                        },
                        TextColor(COL_TEXT_MUTED),
                        Pickable::IGNORE,
                        ParkourBoardRow(idx),
                    ));
                }
            });

            // -----------------------------------------------------------------
            // DYNAMIC CENTER CROSSHAIR RETICLE (Dot + 4 Morphing Brackets)
            // -----------------------------------------------------------------
            hud.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(50.0),
                    top: Val::Percent(50.0),
                    width: Val::Px(24.0),
                    height: Val::Px(24.0),
                    margin: UiRect {
                        left: Val::Px(-12.0),
                        top: Val::Px(-12.0),
                        ..default()
                    },
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                UiPanelKind::Crosshair,
            ))
            .with_children(|ret| {
                // Center dot
                ret.spawn((
                    Node {
                        width: Val::Px(4.0),
                        height: Val::Px(4.0),
                        border_radius: BorderRadius::all(Val::Px(2.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.96, 0.98, 1.0, 0.82)),
                    CrosshairCenterDot,
                ));
                // 4 subtle outer reticle ticks (light up when aiming at interactable or Stalker!)
                for (left, top, w, h) in [
                    (Val::Px(11.0), Val::Px(0.0), Val::Px(2.0), Val::Px(5.0)),
                    (Val::Px(11.0), Val::Px(19.0), Val::Px(2.0), Val::Px(5.0)),
                    (Val::Px(0.0), Val::Px(11.0), Val::Px(5.0), Val::Px(2.0)),
                    (Val::Px(19.0), Val::Px(11.0), Val::Px(5.0), Val::Px(2.0)),
                ] {
                    ret.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left,
                            top,
                            width: w,
                            height: h,
                            border_radius: BorderRadius::all(Val::Px(1.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.22, 0.92, 0.86, 0.0)),
                        CrosshairBracket,
                    ));
                }
            });

            // -----------------------------------------------------------------
            // FLOATING CROSSHAIR LOOK-AT TARGET PILL (Directly under reticle)
            // -----------------------------------------------------------------
            hud.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Percent(54.0),
                    left: Val::Percent(0.0),
                    width: Val::Percent(100.0),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                Visibility::Hidden,
                UiPanelKind::TargetCard,
            ))
            .with_children(|wrap| {
                wrap.spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(9.0),
                        padding: UiRect::axes(Val::Px(12.0), Val::Px(6.0)),
                        border: UiRect::all(Val::Px(1.5)),
                        border_radius: BorderRadius::all(Val::Px(8.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.04, 0.07, 0.11, 0.90)),
                    BorderColor::all(COL_ACCENT_CYAN),
                    TargetCardFrame,
                ))
                .with_children(|pill| {
                    pill.spawn((
                        Node {
                            padding: UiRect::axes(Val::Px(7.0), Val::Px(3.0)),
                            border_radius: BorderRadius::all(Val::Px(4.0)),
                            ..default()
                        },
                        BackgroundColor(COL_ACCENT_CYAN),
                        TargetKeyBadgeBg,
                    ))
                    .with_children(|k| {
                        k.spawn((
                            Text::new("E"),
                            TextFont {
                                font_size: FontSize::Px(12.0),
                                ..default()
                            },
                            TextColor(Color::srgb(0.03, 0.05, 0.07)),
                            TargetKeyBadgeText,
                        ));
                    });
                    pill.spawn(Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(1.0),
                        ..default()
                    })
                    .with_children(|txt| {
                        txt.spawn((
                            Text::new(""),
                            TextFont {
                                font_size: FontSize::Px(13.5),
                                ..default()
                            },
                            TextColor(COL_TEXT_BRIGHT),
                            TargetLabelText,
                        ));
                        txt.spawn((
                            Text::new(""),
                            TextFont {
                                font_size: FontSize::Px(11.0),
                                ..default()
                            },
                            TextColor(COL_TEXT_MUTED),
                            TargetSublabelText,
                        ));
                    });
                });
            });

            // -----------------------------------------------------------------
            // BOTTOM DOCK: Toast Pill + [R] Visual Battery Tray + Graphical Hotbar + Pouch
            // -----------------------------------------------------------------
            hud.spawn(Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(16.0),
                left: Val::Percent(0.0),
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(8.0),
                ..default()
            })
            .with_children(|bottom| {
                // Compact Toast Notification Banner
                bottom
                    .spawn((
                        Node {
                            padding: UiRect::axes(Val::Px(14.0), Val::Px(6.0)),
                            border: UiRect {
                                left: Val::Px(3.0),
                                top: Val::Px(1.0),
                                right: Val::Px(1.0),
                                bottom: Val::Px(1.0),
                            },
                            border_radius: BorderRadius::all(Val::Px(6.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.04, 0.08, 0.12, 0.92)),
                        BorderColor {
                            left: COL_ACCENT_CYAN,
                            top: COL_BORDER_DIM,
                            right: COL_BORDER_DIM,
                            bottom: COL_BORDER_DIM,
                        },
                        Visibility::Hidden,
                        UiPanelKind::ToastCard,
                    ))
                    .with_children(|t| {
                        t.spawn((
                            Text::new(""),
                            TextFont {
                                font_size: FontSize::Px(13.0),
                                ..default()
                            },
                            TextColor(COL_TEXT_BRIGHT),
                            ToastText,
                        ));
                    });

                // Graphical [R] Battery Swap Tray (6 Visual Cell Cards with Vertical Charge Fill!)
                bottom
                    .spawn((
                        Node {
                            padding: UiRect::axes(Val::Px(12.0), Val::Px(9.0)),
                            border: UiRect::all(Val::Px(1.5)),
                            border_radius: BorderRadius::all(Val::Px(9.0)),
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Center,
                            row_gap: Val::Px(7.0),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.04, 0.08, 0.13, 0.95)),
                        BorderColor::all(COL_ACCENT_CYAN),
                        Visibility::Hidden,
                        UiPanelKind::BatteryTray,
                    ))
                    .with_children(|tray| {
                        tray.spawn((
                            Text::new("BATTERY POUCH   [Scroll] Select   [R] Swap   [Q] Drop"),
                            TextFont {
                                font_size: FontSize::Px(11.0),
                                ..default()
                            },
                            TextColor(COL_ACCENT_CYAN),
                        ));
                        tray.spawn(Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(8.0),
                            ..default()
                        })
                        .with_children(|cells| {
                            for idx in 0..MAX_POUCH_CAPACITY {
                                cells
                                    .spawn((
                                        Node {
                                            width: Val::Px(44.0),
                                            height: Val::Px(60.0),
                                            padding: UiRect::all(Val::Px(4.0)),
                                            border: UiRect::all(Val::Px(1.5)),
                                            border_radius: BorderRadius::all(Val::Px(6.0)),
                                            flex_direction: FlexDirection::Column,
                                            align_items: AlignItems::Center,
                                            justify_content: JustifyContent::SpaceBetween,
                                            ..default()
                                        },
                                        BackgroundColor(COL_CARD_BG),
                                        BorderColor::all(COL_BORDER_DIM),
                                        TrayCellCard(idx),
                                    ))
                                    .with_children(|c| {
                                        // Vertical battery tube
                                        c.spawn((
                                            Node {
                                                width: Val::Px(18.0),
                                                height: Val::Px(32.0),
                                                padding: UiRect::all(Val::Px(2.0)),
                                                border: UiRect::all(Val::Px(1.0)),
                                                border_radius: BorderRadius::all(Val::Px(3.0)),
                                                align_items: AlignItems::FlexEnd,
                                                ..default()
                                            },
                                            BackgroundColor(Color::srgba(0.05, 0.07, 0.10, 0.95)),
                                            BorderColor::all(COL_BORDER_DIM),
                                        ))
                                        .with_children(|tube| {
                                            tube.spawn((
                                                Node {
                                                    width: Val::Percent(100.0),
                                                    height: Val::Percent(0.0),
                                                    border_radius: BorderRadius::all(Val::Px(2.0)),
                                                    ..default()
                                                },
                                                BackgroundColor(COL_ACCENT_CYAN),
                                                TrayCellFill(idx),
                                            ));
                                        });
                                        c.spawn((
                                            Text::new("--"),
                                            TextFont {
                                                font_size: FontSize::Px(10.5),
                                                ..default()
                                            },
                                            TextColor(COL_TEXT_BRIGHT),
                                            TrayCellPctText(idx),
                                        ));
                                    });
                            }
                        });
                    });

                // Centered active toolbar item name right above the hotbar
                bottom.spawn((
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(13.0),
                        ..default()
                    },
                    TextColor(COL_TEXT_BRIGHT),
                    HotbarActiveNameText,
                ));

                // Graphical Bottom Equipment Dock: 5 Icon Hotbar Slots + Pouch Gauges + Wearables
                bottom
                    .spawn(Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::FlexEnd,
                        column_gap: Val::Px(10.0),
                        ..default()
                    })
                    .with_children(|dock| {
                        // 5 Graphical Belt Slot Cards
                        dock.spawn(Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(7.0),
                            ..default()
                        })
                        .with_children(|slots| {
                            for idx in 0..MAX_BELT_CAPACITY {
                                slots
                                    .spawn((
                                        Node {
                                            width: Val::Px(64.0),
                                            height: Val::Px(64.0),
                                            border: UiRect::all(Val::Px(2.0)),
                                            border_radius: BorderRadius::all(Val::Px(9.0)),
                                            justify_content: JustifyContent::Center,
                                            align_items: AlignItems::Center,
                                            ..default()
                                        },
                                        BackgroundColor(COL_HUD_POD_BG),
                                        BorderColor::all(COL_BORDER_DIM),
                                        HotbarSlotCard(idx),
                                    ))
                                    .with_children(|slot| {
                                        // Top-left slot keycap number
                                        slot.spawn((
                                            Text::new(format!("{}", idx + 1)),
                                            TextFont {
                                                font_size: FontSize::Px(10.5),
                                                ..default()
                                            },
                                            TextColor(COL_TEXT_MUTED),
                                            Node {
                                                position_type: PositionType::Absolute,
                                                top: Val::Px(4.0),
                                                left: Val::Px(6.0),
                                                ..default()
                                            },
                                        ));

                                        // Procedural Graphical Icon (Primary + Secondary shape)
                                        slot.spawn((
                                            Node {
                                                width: Val::Px(24.0),
                                                height: Val::Px(24.0),
                                                border_radius: BorderRadius::all(Val::Px(5.0)),
                                                justify_content: JustifyContent::Center,
                                                align_items: AlignItems::Center,
                                                ..default()
                                            },
                                            BackgroundColor(Color::NONE),
                                            HotbarIconPrimary(idx),
                                        ))
                                        .with_children(|icon| {
                                            icon.spawn((
                                                Node {
                                                    width: Val::Px(10.0),
                                                    height: Val::Px(10.0),
                                                    border_radius: BorderRadius::all(Val::Px(2.0)),
                                                    ..default()
                                                },
                                                BackgroundColor(Color::NONE),
                                                HotbarIconSecondary(idx),
                                            ));
                                        });

                                        // Bottom-right count / short tag badge
                                        slot.spawn((
                                            Text::new(""),
                                            TextFont {
                                                font_size: FontSize::Px(11.0),
                                                ..default()
                                            },
                                            TextColor(COL_TEXT_BRIGHT),
                                            Node {
                                                position_type: PositionType::Absolute,
                                                bottom: Val::Px(5.0),
                                                right: Val::Px(6.0),
                                                ..default()
                                            },
                                            HotbarCountText(idx),
                                        ));

                                        // Bottom thin charge strip (for Flashlight slot)
                                        slot.spawn((
                                            Node {
                                                position_type: PositionType::Absolute,
                                                bottom: Val::Px(2.0),
                                                left: Val::Px(6.0),
                                                width: Val::Px(48.0),
                                                height: Val::Px(3.0),
                                                border_radius: BorderRadius::all(Val::Px(2.0)),
                                                ..default()
                                            },
                                            BackgroundColor(Color::NONE),
                                        ))
                                        .with_children(|bar| {
                                            bar.spawn((
                                                Node {
                                                    width: Val::Percent(0.0),
                                                    height: Val::Percent(100.0),
                                                    border_radius: BorderRadius::all(Val::Px(2.0)),
                                                    ..default()
                                                },
                                                BackgroundColor(COL_ACCENT_CYAN),
                                                HotbarChargeFill(idx),
                                            ));
                                        });
                                    });
                            }
                        });

                        // Right of Hotbar: Mini Battery Pouch Vertical Gauges + Wearable Badges
                        dock.spawn((
                            Node {
                                padding: UiRect::axes(Val::Px(9.0), Val::Px(8.0)),
                                border: UiRect::all(Val::Px(1.5)),
                                border_radius: BorderRadius::all(Val::Px(8.0)),
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(5.0),
                                ..default()
                            },
                            BackgroundColor(COL_HUD_POD_BG),
                            BorderColor::all(COL_BORDER_DIM),
                        ))
                        .with_children(|side| {
                            // 4 Mini Battery Pouch Gauges
                            side.spawn(Node {
                                flex_direction: FlexDirection::Row,
                                column_gap: Val::Px(4.0),
                                align_items: AlignItems::FlexEnd,
                                ..default()
                            })
                            .with_children(|pouch| {
                                for idx in 0..4 {
                                    pouch
                                        .spawn((
                                            Node {
                                                width: Val::Px(10.0),
                                                height: Val::Px(22.0),
                                                padding: UiRect::all(Val::Px(1.5)),
                                                border: UiRect::all(Val::Px(1.0)),
                                                border_radius: BorderRadius::all(Val::Px(2.0)),
                                                align_items: AlignItems::FlexEnd,
                                                ..default()
                                            },
                                            BackgroundColor(Color::srgba(0.06, 0.08, 0.12, 0.9)),
                                            BorderColor::all(COL_BORDER_DIM),
                                        ))
                                        .with_children(|g| {
                                            g.spawn((
                                                Node {
                                                    width: Val::Percent(100.0),
                                                    height: Val::Percent(0.0),
                                                    ..default()
                                                },
                                                BackgroundColor(COL_ACCENT_CYAN),
                                                PouchGaugeFill(idx),
                                            ));
                                        });
                                }
                            });

                            // 2 Compact Wearable Badges (HL = Headlamp, SB = Stealth Boots)
                            side.spawn(Node {
                                flex_direction: FlexDirection::Row,
                                column_gap: Val::Px(5.0),
                                ..default()
                            })
                            .with_children(|wear| {
                                for (tag, is_headlamp) in [("HL", true), ("SB", false)] {
                                    wear.spawn((
                                        Node {
                                            min_width: Val::Px(if is_headlamp { 48.0 } else { 24.0 }),
                                            height: Val::Px(18.0),
                                            padding: UiRect::axes(Val::Px(4.0), Val::Px(0.0)),
                                            border: UiRect::all(Val::Px(1.0)),
                                            border_radius: BorderRadius::all(Val::Px(4.0)),
                                            justify_content: JustifyContent::Center,
                                            align_items: AlignItems::Center,
                                            ..default()
                                        },
                                        BackgroundColor(Color::srgba(0.08, 0.10, 0.14, 0.75)),
                                        BorderColor::all(COL_BORDER_DIM),
                                        WearableBadge { is_headlamp },
                                    ))
                                    .with_children(|b| {
                                        let txt = b
                                            .spawn((
                                                Text::new(tag),
                                                TextFont {
                                                    font_size: FontSize::Px(9.5),
                                                    ..default()
                                                },
                                                TextColor(COL_TEXT_BRIGHT),
                                            ))
                                            .id();
                                        if is_headlamp {
                                            b.commands().entity(txt).insert(HeadlampBadgeText);
                                        }
                                    });
                                }
                            });
                        });
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
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.015, 0.025, 0.04, 0.62)),
            GlobalZIndex(40),
            Visibility::Hidden,
            UiPanelKind::CoopMenu,
        ))
        .with_children(|backdrop| {
            backdrop
                .spawn((
                    Node {
                        width: Val::Px(540.0),
                        padding: UiRect::all(Val::Px(22.0)),
                        border: UiRect {
                            top: Val::Px(3.0),
                            left: Val::Px(1.5),
                            right: Val::Px(1.5),
                            bottom: Val::Px(1.5),
                        },
                        border_radius: BorderRadius::all(Val::Px(12.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(12.0),
                        ..default()
                    },
                    BackgroundColor(COL_PANEL_BG),
                    BorderColor {
                        top: COL_ACCENT_CYAN,
                        left: COL_BORDER_DIM,
                        right: COL_BORDER_DIM,
                        bottom: COL_BORDER_DIM,
                    },
                ))
                .with_children(|panel| {
                    panel.spawn((
                        Text::new("EXPEDITION MENU"),
                        TextFont {
                            font_size: FontSize::Px(20.0),
                            ..default()
                        },
                        TextColor(COL_ACCENT_CYAN),
                    ));

                    panel
                        .spawn((
                            Node {
                                width: Val::Percent(100.0),
                                padding: UiRect::all(Val::Px(12.0)),
                                border_radius: BorderRadius::all(Val::Px(8.0)),
                                ..default()
                            },
                            BackgroundColor(COL_CARD_BG),
                        ))
                        .with_children(|box_n| {
                            box_n.spawn((
                                Text::new("Status: Offline"),
                                TextFont {
                                    font_size: FontSize::Px(13.0),
                                    ..default()
                                },
                                TextColor(COL_TEXT_BRIGHT),
                                CoopPanelStatusText,
                            ));
                        });

                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: Val::Px(8.0),
                            row_gap: Val::Px(6.0),
                            ..default()
                        })
                        .with_children(|row| {
                            spawn_std_btn(row, "Copy Join Code", UiAction::CopyJoinCode);
                            spawn_std_btn(row, "Cycle Visor", UiAction::CycleVisor);
                            spawn_toggle_btn(
                                row,
                                "Mic: LIVE [M]",
                                UiAction::ToggleMute,
                                ToggleKind::Mute,
                            );
                            spawn_toggle_btn(
                                row,
                                "Self-Echo: OFF [V]",
                                UiAction::ToggleSelfEcho,
                                ToggleKind::SelfEcho,
                            );
                        });

                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            justify_content: JustifyContent::SpaceBetween,
                            width: Val::Percent(100.0),
                            margin: UiRect::top(Val::Px(4.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn(Node {
                                flex_direction: FlexDirection::Row,
                                column_gap: Val::Px(8.0),
                                ..default()
                            })
                            .with_children(|left| {
                                spawn_std_btn(left, "Settings", UiAction::OpenSettings);
                                spawn_std_btn(left, "Save & Menu", UiAction::DisconnectToMenu);
                                spawn_danger_btn(left, "Quit (No Save)", UiAction::QuitNoSave);
                            });
                            spawn_accent_btn(row, "Resume", UiAction::ResumeGame);
                        });
                });
        });

    // =========================================================================
    // 5. GRAPHICAL BASE SHED SUPPLY SHOP (2-Column Icon Cards + Tier Tabs)
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
            BackgroundColor(Color::srgba(0.015, 0.025, 0.04, 0.68)),
            GlobalZIndex(30),
            Visibility::Hidden,
            UiPanelKind::ShopOverlay,
        ))
        .with_children(|backdrop| {
            backdrop
                .spawn((
                    Node {
                        width: Val::Px(720.0),
                        padding: UiRect::all(Val::Px(20.0)),
                        border: UiRect {
                            top: Val::Px(3.0),
                            left: Val::Px(1.5),
                            right: Val::Px(1.5),
                            bottom: Val::Px(1.5),
                        },
                        border_radius: BorderRadius::all(Val::Px(12.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(12.0),
                        ..default()
                    },
                    BackgroundColor(COL_PANEL_BG),
                    BorderColor {
                        top: COL_ACCENT_GREEN,
                        left: COL_BORDER_DIM,
                        right: COL_BORDER_DIM,
                        bottom: COL_BORDER_DIM,
                    },
                ))
                .with_children(|panel| {
                    // Top Bar: Title + Save Badge + Large Golden Credits Pill
                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            justify_content: JustifyContent::SpaceBetween,
                            align_items: AlignItems::Center,
                            width: Val::Percent(100.0),
                            ..default()
                        })
                        .with_children(|hdr| {
                            hdr.spawn(Node {
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(2.0),
                                ..default()
                            })
                            .with_children(|l| {
                                l.spawn((
                                    Text::new("SUPPLY TERMINAL"),
                                    TextFont {
                                        font_size: FontSize::Px(20.0),
                                        ..default()
                                    },
                                    TextColor(COL_ACCENT_GREEN),
                                ));
                                l.spawn((
                                    Text::new("Save: default"),
                                    TextFont {
                                        font_size: FontSize::Px(11.5),
                                        ..default()
                                    },
                                    TextColor(COL_TEXT_MUTED),
                                    ShopSaveTagText,
                                ));
                            });

                            hdr.spawn((
                                Node {
                                    padding: UiRect::axes(Val::Px(14.0), Val::Px(7.0)),
                                    border: UiRect::all(Val::Px(1.5)),
                                    border_radius: BorderRadius::all(Val::Px(8.0)),
                                    ..default()
                                },
                                BackgroundColor(Color::srgba(0.16, 0.13, 0.05, 0.95)),
                                BorderColor::all(COL_ACCENT_AMBER),
                            ))
                            .with_children(|cr| {
                                cr.spawn((
                                    Text::new("100 cr"),
                                    TextFont {
                                        font_size: FontSize::Px(17.0),
                                        ..default()
                                    },
                                    TextColor(COL_ACCENT_AMBER),
                                    ShopCreditsText,
                                ));
                            });
                        });

                    // Visual Depth Tier Selector Tabs
                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(8.0),
                            width: Val::Percent(100.0),
                            ..default()
                        })
                        .with_children(|tabs| {
                            for tier in 1..=3u8 {
                                let label = format!("DEPTH TIER {tier}");
                                tabs.spawn((
                                    Node {
                                        flex_grow: 1.0,
                                        padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                                        border: UiRect::all(Val::Px(1.5)),
                                        border_radius: BorderRadius::all(Val::Px(7.0)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        ..default()
                                    },
                                    BackgroundColor(COL_CARD_BG),
                                    BorderColor::all(COL_BORDER_DIM),
                                    ShopTierTab(tier),
                                ))
                                .observe(
                                    move |_click: On<PointerClick>,
                                          mut writer: MessageWriter<TriggerUiAction>| {
                                        writer.write(TriggerUiAction(UiAction::ShopTier(tier)));
                                    },
                                )
                                .with_children(|t| {
                                    t.spawn((
                                        Text::new(label),
                                        TextFont {
                                            font_size: FontSize::Px(12.5),
                                            ..default()
                                        },
                                        TextColor(COL_TEXT_BRIGHT),
                                    ));
                                });
                            }
                        });

                    // 2-Column Graphical Gear Card Grid
                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: Val::Px(10.0),
                            row_gap: Val::Px(8.0),
                            width: Val::Percent(100.0),
                            ..default()
                        })
                        .with_children(|grid| {
                            for entry in SHOP_CATALOG {
                                let (accent, code) = shop_item_visual_accent(entry.item);
                                let action = UiAction::ShopBuy(entry.item);
                                let base_bg = COL_CARD_BG;
                                let hover_bg = Color::srgba(0.13, 0.19, 0.27, 0.98);
                                grid.spawn((
                                    Node {
                                        width: Val::Px(332.0),
                                        padding: UiRect::axes(Val::Px(10.0), Val::Px(9.0)),
                                        border: UiRect::all(Val::Px(1.5)),
                                        border_radius: BorderRadius::all(Val::Px(8.0)),
                                        flex_direction: FlexDirection::Row,
                                        align_items: AlignItems::Center,
                                        justify_content: JustifyContent::SpaceBetween,
                                        column_gap: Val::Px(10.0),
                                        ..default()
                                    },
                                    BackgroundColor(base_bg),
                                    BorderColor::all(COL_BORDER_DIM),
                                    action,
                                ))
                                .observe(
                                    move |_click: On<PointerClick>,
                                          mut writer: MessageWriter<TriggerUiAction>| {
                                        writer.write(TriggerUiAction(action));
                                    },
                                )
                                .observe(
                                    move |over: On<PointerOver>,
                                          mut bg_q: Query<&mut BackgroundColor>| {
                                        if let Ok(mut bg) = bg_q.get_mut(over.entity) {
                                            *bg = BackgroundColor(hover_bg);
                                        }
                                    },
                                )
                                .observe(
                                    move |out: On<PointerOut>,
                                          mut bg_q: Query<&mut BackgroundColor>| {
                                        if let Ok(mut bg) = bg_q.get_mut(out.entity) {
                                            *bg = BackgroundColor(base_bg);
                                        }
                                    },
                                )
                                .with_children(|card| {
                                    card.spawn(Node {
                                        flex_direction: FlexDirection::Row,
                                        align_items: AlignItems::Center,
                                        column_gap: Val::Px(10.0),
                                        ..default()
                                    })
                                    .with_children(|left| {
                                        // Color-coded visual icon badge
                                        left.spawn((
                                            Node {
                                                width: Val::Px(36.0),
                                                height: Val::Px(36.0),
                                                border: UiRect::all(Val::Px(1.5)),
                                                border_radius: BorderRadius::all(Val::Px(7.0)),
                                                justify_content: JustifyContent::Center,
                                                align_items: AlignItems::Center,
                                                ..default()
                                            },
                                            BackgroundColor(Color::srgba(0.05, 0.08, 0.12, 0.96)),
                                            BorderColor::all(accent),
                                        ))
                                        .with_children(|ic| {
                                            ic.spawn((
                                                Text::new(code),
                                                TextFont {
                                                    font_size: FontSize::Px(11.0),
                                                    ..default()
                                                },
                                                TextColor(accent),
                                            ));
                                        });

                                        left.spawn(Node {
                                            flex_direction: FlexDirection::Column,
                                            row_gap: Val::Px(2.0),
                                            ..default()
                                        })
                                        .with_children(|txt| {
                                            txt.spawn((
                                                Text::new(entry.name),
                                                TextFont {
                                                    font_size: FontSize::Px(13.5),
                                                    ..default()
                                                },
                                                TextColor(COL_TEXT_BRIGHT),
                                            ));
                                            txt.spawn((
                                                Text::new(entry.description),
                                                TextFont {
                                                    font_size: FontSize::Px(10.5),
                                                    ..default()
                                                },
                                                TextColor(COL_TEXT_MUTED),
                                            ));
                                        });
                                    });

                                    // Right-aligned golden price pill
                                    card.spawn((
                                        Node {
                                            padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                                            border_radius: BorderRadius::all(Val::Px(5.0)),
                                            ..default()
                                        },
                                        BackgroundColor(Color::srgba(0.16, 0.13, 0.05, 0.92)),
                                    ))
                                    .with_children(|pr| {
                                        pr.spawn((
                                            Text::new(format!("{} cr", entry.cost)),
                                            TextFont {
                                                font_size: FontSize::Px(12.0),
                                                ..default()
                                            },
                                            TextColor(COL_ACCENT_AMBER),
                                        ));
                                    });
                                });
                            }
                        });

                    // Footer Controls
                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            justify_content: JustifyContent::SpaceBetween,
                            width: Val::Percent(100.0),
                            margin: UiRect::top(Val::Px(2.0)),
                            ..default()
                        })
                        .with_children(|foot| {
                            spawn_accent_btn(foot, "Save Campaign", UiAction::ShopManualSave);
                            spawn_danger_btn(foot, "Close [B / Esc]", UiAction::ShopClose);
                        });
                });
        });

    // =========================================================================
    // 6. MAIN MENU AUTO-UPDATE POPUP MODAL (GlobalZIndex(60))
    // =========================================================================
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.01, 0.02, 0.035, 0.82)),
            GlobalZIndex(60),
            Visibility::Hidden,
            UiPanelKind::UpdatePopup,
        ))
        .with_children(|backdrop| {
            backdrop
                .spawn((
                    Node {
                        width: Val::Px(460.0),
                        padding: UiRect::all(Val::Px(24.0)),
                        border: UiRect {
                            top: Val::Px(3.0),
                            left: Val::Px(1.5),
                            right: Val::Px(1.5),
                            bottom: Val::Px(1.5),
                        },
                        border_radius: BorderRadius::all(Val::Px(12.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(14.0),
                        ..default()
                    },
                    BackgroundColor(COL_PANEL_BG),
                    BorderColor {
                        top: COL_ACCENT_CYAN,
                        left: COL_BORDER_DIM,
                        right: COL_BORDER_DIM,
                        bottom: COL_BORDER_DIM,
                    },
                ))
                .with_children(|modal| {
                    // Clean stacked header: Title + Version delta subtitle
                    modal
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(4.0),
                            ..default()
                        })
                        .with_children(|hdr| {
                            hdr.spawn((
                                Text::new("Update Available"),
                                TextFont {
                                    font_size: FontSize::Px(22.0),
                                    ..default()
                                },
                                TextColor(COL_ACCENT_CYAN),
                            ));
                            hdr.spawn((
                                Text::new(format!("{APP_VERSION}  →  ...")),
                                TextFont {
                                    font_size: FontSize::Px(14.0),
                                    ..default()
                                },
                                TextColor(COL_TEXT_MUTED),
                                UpdatePopupVersionText,
                            ));
                        });

                    // Clean, airy highlights box (bulleted, stripped of markdown/URL clutter)
                    modal
                        .spawn((
                            Node {
                                width: Val::Percent(100.0),
                                max_height: Val::Px(170.0),
                                padding: UiRect::all(Val::Px(14.0)),
                                border: UiRect::all(Val::Px(1.0)),
                                border_radius: BorderRadius::all(Val::Px(8.0)),
                                overflow: Overflow::clip(),
                                ..default()
                            },
                            BackgroundColor(COL_CARD_BG),
                            BorderColor::all(COL_BORDER_DIM),
                        ))
                        .with_children(|notes_box| {
                            notes_box.spawn((
                                Text::new(""),
                                TextFont {
                                    font_size: FontSize::Px(13.5),
                                    ..default()
                                },
                                TextColor(COL_TEXT_BRIGHT),
                                UpdatePopupNotesText,
                            ));
                        });

                    // Progress bar & status line (hidden until Update is clicked or an error occurs!)
                    modal
                        .spawn((
                            Node {
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(6.0),
                                width: Val::Percent(100.0),
                                display: Display::None,
                                ..default()
                            },
                            UpdatePopupProgressWrap,
                        ))
                        .with_children(|prog| {
                            prog.spawn((
                                Node {
                                    width: Val::Percent(100.0),
                                    height: Val::Px(7.0),
                                    border_radius: BorderRadius::all(Val::Px(4.0)),
                                    overflow: Overflow::clip(),
                                    ..default()
                                },
                                BackgroundColor(Color::srgba(0.11, 0.16, 0.22, 0.95)),
                            ))
                            .with_children(|track| {
                                track.spawn((
                                    Node {
                                        width: Val::Percent(0.0),
                                        height: Val::Percent(100.0),
                                        border_radius: BorderRadius::all(Val::Px(4.0)),
                                        ..default()
                                    },
                                    BackgroundColor(COL_ACCENT_CYAN),
                                    UpdatePopupProgressFill,
                                ));
                            });

                            prog.spawn((
                                Text::new(""),
                                TextFont {
                                    font_size: FontSize::Px(13.0),
                                    ..default()
                                },
                                TextColor(COL_TEXT_MUTED),
                                UpdatePopupStatusText,
                            ));
                        });

                    // Simple two-button footer: [Not Now]  [Update & Restart]
                    modal
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            justify_content: JustifyContent::SpaceBetween,
                            width: Val::Percent(100.0),
                            margin: UiRect::top(Val::Px(4.0)),
                            ..default()
                        })
                        .with_children(|btns| {
                            let dismiss_txt = spawn_std_btn(
                                btns,
                                &format!("Stay on {APP_VERSION}"),
                                UiAction::UpdateDismiss,
                            );
                            btns.commands()
                                .entity(dismiss_txt)
                                .insert(UpdatePopupDismissLabel);

                            let install_txt = spawn_accent_btn(
                                btns,
                                "Update & Restart",
                                UiAction::UpdateInstall,
                            );
                            btns.commands()
                                .entity(install_txt)
                                .insert(UpdatePopupInstallLabel);
                        });
                });
        });

    // =========================================================================
    // 7. MAIN MENU SAVE CREATE / RENAME MODAL (GlobalZIndex(55))
    // =========================================================================
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::srgba(0.01, 0.02, 0.035, 0.78)),
            GlobalZIndex(55),
            Visibility::Hidden,
            UiPanelKind::SaveNameModal,
        ))
        .observe(
            |_click: On<PointerClick>, mut writer: MessageWriter<TriggerUiAction>| {
                writer.write(TriggerUiAction(UiAction::BlurSaveNameInput));
            },
        )
        .with_children(|backdrop| {
            backdrop
                .spawn((
                    Node {
                        width: Val::Px(430.0),
                        padding: UiRect::all(Val::Px(22.0)),
                        border: UiRect {
                            top: Val::Px(3.0),
                            left: Val::Px(1.5),
                            right: Val::Px(1.5),
                            bottom: Val::Px(1.5),
                        },
                        border_radius: BorderRadius::all(Val::Px(12.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(12.0),
                        ..default()
                    },
                    BackgroundColor(COL_PANEL_BG),
                    BorderColor {
                        top: COL_ACCENT_CYAN,
                        left: COL_BORDER_DIM,
                        right: COL_BORDER_DIM,
                        bottom: COL_BORDER_DIM,
                    },
                ))
                .observe(
                    |mut click: On<PointerClick>, mut writer: MessageWriter<TriggerUiAction>| {
                        click.propagate(false);
                        writer.write(TriggerUiAction(UiAction::BlurSaveNameInput));
                    },
                )
                .with_children(|modal| {
                    modal
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(3.0),
                            ..default()
                        })
                        .with_children(|hdr| {
                            hdr.spawn((
                                Text::new("New Save"),
                                TextFont {
                                    font_size: FontSize::Px(20.0),
                                    ..default()
                                },
                                TextColor(COL_ACCENT_CYAN),
                                SaveNameModalTitle,
                            ));
                            hdr.spawn((
                                Text::new("Enter a name for your new expedition save"),
                                TextFont {
                                    font_size: FontSize::Px(13.0),
                                    ..default()
                                },
                                TextColor(COL_TEXT_MUTED),
                                SaveNameModalSubtitle,
                            ));
                        });

                    modal
                        .spawn((
                            Node {
                                width: Val::Percent(100.0),
                                padding: UiRect::axes(Val::Px(12.0), Val::Px(10.0)),
                                border: UiRect::all(Val::Px(2.0)),
                                border_radius: BorderRadius::all(Val::Px(6.0)),
                                flex_direction: FlexDirection::Row,
                                justify_content: JustifyContent::SpaceBetween,
                                align_items: AlignItems::Center,
                                column_gap: Val::Px(8.0),
                                ..default()
                            },
                            BackgroundColor(Color::srgba(0.06, 0.14, 0.20, 0.98)),
                            BorderColor::all(COL_ACCENT_CYAN),
                            SaveNameInputBox,
                        ))
                        .observe(
                            |mut click: On<PointerClick>,
                             mut writer: MessageWriter<TriggerUiAction>| {
                                click.propagate(false);
                                writer.write(TriggerUiAction(UiAction::FocusSaveNameInput));
                            },
                        )
                        .with_children(|input_box| {
                            input_box.spawn((
                                Text::new("|"),
                                TextFont {
                                    font_size: FontSize::Px(15.0),
                                    ..default()
                                },
                                TextColor(COL_TEXT_BRIGHT),
                                Pickable::IGNORE,
                                SaveNameInputText,
                            ));
                            input_box.spawn((
                                Text::new("TYPING"),
                                TextFont {
                                    font_size: FontSize::Px(10.5),
                                    ..default()
                                },
                                TextColor(COL_ACCENT_CYAN),
                                Pickable::IGNORE,
                                SaveNameFocusBadge,
                            ));
                        });

                    modal.spawn((
                        Text::new("Enter to confirm • Esc to cancel • Del to clear"),
                        TextFont {
                            font_size: FontSize::Px(12.0),
                            ..default()
                        },
                        TextColor(COL_TEXT_MUTED),
                        SaveNameHintText,
                    ));

                    modal
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            justify_content: JustifyContent::SpaceBetween,
                            width: Val::Percent(100.0),
                            margin: UiRect::top(Val::Px(4.0)),
                            ..default()
                        })
                        .with_children(|btns| {
                            spawn_std_btn(btns, "Cancel", UiAction::CancelSaveNamePrompt);
                            let confirm_txt = spawn_accent_btn(
                                btns,
                                "Create Save",
                                UiAction::ConfirmSaveNamePrompt,
                            );
                            btns.commands()
                                .entity(confirm_txt)
                                .insert(SaveNameConfirmLabel);
                        });
                });
        });
}

// =============================================================================
// INPUT & ACTION HANDLERS
// =============================================================================

#[allow(clippy::too_many_arguments)]
fn handle_name_typing_system(
    time: Res<Time>,
    screen: Res<AppScreen>,
    settings_open: Res<SettingsMenuOpen>,
    keys: Res<ButtonInput<KeyCode>>,
    mut focused: ResMut<NameInputFocused>,
    mut save_prompt: ResMut<SaveNamePromptState>,
    mut key_events: MessageReader<KeyboardInput>,
    mut ui_writer: MessageWriter<TriggerUiAction>,
    mut user_settings: ResMut<UserSettings>,
    mut coop: ResMut<CoopState>,
) {
    if *screen != AppScreen::MainMenu || settings_open.0 {
        key_events.clear();
        return;
    }

    let now_s = time.elapsed_secs();

    if save_prompt.open {
        let ctrl_held =
            keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
        for ev in key_events.read() {
            if ev.state != ButtonState::Pressed {
                continue;
            }
            match &ev.logical_key {
                Key::Enter => {
                    ui_writer.write(TriggerUiAction(UiAction::ConfirmSaveNamePrompt));
                }
                Key::Escape => {
                    ui_writer.write(TriggerUiAction(UiAction::CancelSaveNamePrompt));
                }
                Key::Tab => {
                    save_prompt.focused = true;
                    save_prompt.caret_reset_s = now_s;
                }
                Key::Backspace if save_prompt.focused => {
                    if ctrl_held {
                        save_prompt.buffer.clear();
                    } else {
                        save_prompt.buffer.pop();
                    }
                    save_prompt.error_text.clear();
                    save_prompt.caret_reset_s = now_s;
                }
                Key::Delete if save_prompt.focused => {
                    save_prompt.buffer.clear();
                    save_prompt.error_text.clear();
                    save_prompt.caret_reset_s = now_s;
                }
                Key::Space if save_prompt.focused => {
                    if save_prompt.buffer.len() < MAX_SAVE_SLOT_NAME_LEN {
                        save_prompt.buffer.push(' ');
                        save_prompt.error_text.clear();
                        save_prompt.caret_reset_s = now_s;
                    }
                }
                Key::Character(s) if save_prompt.focused => {
                    for ch in s.chars() {
                        if save_prompt.buffer.len() < MAX_SAVE_SLOT_NAME_LEN
                            && (ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == ' ')
                        {
                            save_prompt.buffer.push(ch);
                            save_prompt.error_text.clear();
                            save_prompt.caret_reset_s = now_s;
                        }
                    }
                }
                _ => {}
            }
        }
        return;
    }

    if !focused.0 {
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
        SliderKind::Fov => {
            ((settings.fov_deg - FOV_MIN_DEG) / (FOV_MAX_DEG - FOV_MIN_DEG)).clamp(0.0, 1.0)
        }
        SliderKind::MasterVolume => (settings.master_volume / 1.5).clamp(0.0, 1.0),
        SliderKind::VoiceVolume => (settings.voice_volume / 2.0).clamp(0.0, 1.0),
        SliderKind::MicGain => ((settings.mic_gain - 0.2) / 2.8).clamp(0.0, 1.0),
        SliderKind::MicGate => ((settings.gate_threshold_db + 65.0) / 50.0).clamp(0.0, 1.0),
    }
}

#[derive(SystemParam)]
struct ActionResources<'w> {
    time: Res<'w, Time>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    menu_open: ResMut<'w, CoopMenuOpen>,
    settings_open: ResMut<'w, SettingsMenuOpen>,
    diag_open: ResMut<'w, DiagnosticsDrawerOpen>,
    name_focused: ResMut<'w, NameInputFocused>,
    coop: ResMut<'w, CoopState>,
    user_settings: ResMut<'w, UserSettings>,
    voice_settings: ResMut<'w, VoiceSettings>,
    device_catalog: ResMut<'w, AudioDeviceCatalog>,
    save_browser: ResMut<'w, SaveSlotBrowser>,
    save_dropdown: ResMut<'w, SaveDropdownState>,
    save_prompt: ResMut<'w, SaveNamePromptState>,
    campaign: ResMut<'w, CampaignState>,
    audio_dropdown: ResMut<'w, AudioDropdownState>,
    inv: Res<'w, PlayerInventory>,
}

#[allow(clippy::too_many_arguments)]
fn handle_ui_actions_system(
    mut commands: Commands,
    mut actions: MessageReader<TriggerUiAction>,
    mut res: ActionResources,
    mut lifecycle_writer: MessageWriter<SessionLifecycleEvent>,
    mut shop_writer: MessageWriter<ShopActionEvent>,
    mut updater_writer: MessageWriter<UpdaterCommand>,
    mut remove_voice: MessageWriter<RemoveVoiceSpeakerEvent>,
    mut exit_writer: MessageWriter<AppExit>,
) {
    if res.keys.just_pressed(KeyCode::F3) {
        res.diag_open.0 = !res.diag_open.0;
    }

    let menu_open = &mut *res.menu_open;
    let settings_open = &mut *res.settings_open;
    let name_focused = &mut *res.name_focused;
    let coop = &mut *res.coop;
    let user_settings = &mut *res.user_settings;
    let voice_settings = &mut *res.voice_settings;
    let device_catalog = &mut *res.device_catalog;
    let save_browser = &mut *res.save_browser;
    let save_dropdown = &mut *res.save_dropdown;
    let save_prompt = &mut *res.save_prompt;
    let campaign = &mut *res.campaign;
    let audio_dropdown = &mut *res.audio_dropdown;
    let inv = &*res.inv;

    for TriggerUiAction(action) in actions.read().copied() {
        if !matches!(action, UiAction::FocusNameInput) {
            name_focused.0 = false;
        }
        match action {
            UiAction::FocusNameInput => name_focused.0 = true,
            UiAction::SelectVisor(idx) => {
                let clamped = idx % VISOR_PALETTE.len();
                user_settings.visor_idx = clamped;
                coop.visor_idx = clamped;
                if coop.is_connected() {
                    coop.sync_local_profile(inv);
                }
            }
            UiAction::ToggleSaveDropdown => {
                save_dropdown.open = !save_dropdown.open;
                if save_dropdown.open {
                    save_browser.refresh();
                    save_dropdown.page = save_browser.selected_idx / SAVE_DROPDOWN_ROWS;
                }
            }
            UiAction::SelectSaveRow(row_idx) => {
                let abs_idx = save_dropdown.page * SAVE_DROPDOWN_ROWS + row_idx;
                if abs_idx < save_browser.slots.len() {
                    save_browser.selected_idx = abs_idx;
                    coop.status_text =
                        format!("Save slot '{}' selected", save_browser.selected_slot_name());
                }
                save_dropdown.open = false;
            }
            UiAction::SaveListPage(delta) => {
                let total_pages = save_browser.slots.len().div_ceil(SAVE_DROPDOWN_ROWS).max(1);
                let next = (save_dropdown.page as i32 + delta).rem_euclid(total_pages as i32);
                save_dropdown.page = next as usize;
            }
            UiAction::RefreshSaveSlots => {
                save_browser.refresh();
                save_dropdown.page = save_browser.selected_idx / SAVE_DROPDOWN_ROWS;
                coop.status_text = format!(
                    "Found {} save slot(s) — '{}'",
                    save_browser.slots.len(),
                    save_browser.selected_slot_name()
                );
            }
            UiAction::OpenNewSavePrompt => {
                save_browser.refresh();
                save_dropdown.open = false;
                save_prompt.open = true;
                save_prompt.focused = true;
                save_prompt.caret_reset_s = res.time.elapsed_secs();
                save_prompt.mode = Some(SaveNamePromptMode::Create);
                save_prompt.buffer.clear();
                save_prompt.default_hint = save_browser.next_default_new_slot_name();
                save_prompt.target_old_name.clear();
                save_prompt.error_text.clear();
            }
            UiAction::OpenRenameSavePrompt => {
                save_browser.refresh();
                save_dropdown.open = false;
                let current = save_browser.selected_slot_name().to_string();
                save_prompt.open = true;
                save_prompt.focused = true;
                save_prompt.caret_reset_s = res.time.elapsed_secs();
                save_prompt.mode = Some(SaveNamePromptMode::Rename);
                save_prompt.buffer = current.clone();
                save_prompt.default_hint.clear();
                save_prompt.target_old_name = current;
                save_prompt.error_text.clear();
            }
            UiAction::FocusSaveNameInput => {
                if save_prompt.open {
                    save_prompt.focused = true;
                    save_prompt.caret_reset_s = res.time.elapsed_secs();
                }
            }
            UiAction::BlurSaveNameInput => {
                save_prompt.focused = false;
            }
            UiAction::CancelSaveNamePrompt => {
                save_prompt.open = false;
                save_prompt.focused = false;
                save_prompt.error_text.clear();
            }
            UiAction::ConfirmSaveNamePrompt => match save_prompt.mode {
                Some(SaveNamePromptMode::Create) => {
                    match save_browser.create_named_slot(&save_prompt.buffer) {
                        Ok(name) => {
                            save_prompt.open = false;
                            save_prompt.error_text.clear();
                            save_dropdown.page = save_browser.selected_idx / SAVE_DROPDOWN_ROWS;
                            save_dropdown.open = false;
                            coop.status_text = format!("Created new save '{name}'");
                        }
                        Err(e) => {
                            save_prompt.error_text = e;
                        }
                    }
                }
                Some(SaveNamePromptMode::Rename) => {
                    let old_name = save_prompt.target_old_name.clone();
                    match save_browser.rename_selected_slot(&save_prompt.buffer) {
                        Ok(new_name) => {
                            if campaign.slot_name == old_name {
                                campaign.slot_name = new_name.clone();
                            }
                            save_prompt.open = false;
                            save_prompt.error_text.clear();
                            save_dropdown.page = save_browser.selected_idx / SAVE_DROPDOWN_ROWS;
                            save_dropdown.open = false;
                            coop.status_text = if old_name == new_name {
                                format!("Save slot '{}' unchanged", new_name)
                            } else {
                                format!("Renamed save '{old_name}' → '{new_name}'")
                            };
                        }
                        Err(e) => {
                            save_prompt.error_text = e;
                        }
                    }
                }
                None => {
                    save_prompt.open = false;
                }
            },
            UiAction::PlaySolo => {
                save_prompt.open = false;
                user_settings.sanitize();
                coop.local_name = user_settings.player_name.clone();
                coop.visor_idx = user_settings.visor_idx % VISOR_PALETTE.len();
                lifecycle_writer.write(SessionLifecycleEvent::StartSolo);
            }
            UiAction::HostLobby => {
                save_prompt.open = false;
                user_settings.sanitize();
                coop.local_name = user_settings.player_name.clone();
                coop.visor_idx = user_settings.visor_idx % VISOR_PALETTE.len();
                lifecycle_writer.write(SessionLifecycleEvent::StartHost);
            }
            UiAction::PasteAndJoin => {
                save_prompt.open = false;
                user_settings.sanitize();
                coop.local_name = user_settings.player_name.clone();
                coop.visor_idx = user_settings.visor_idx % VISOR_PALETTE.len();
                clear_remote_players(&mut commands, coop, &mut remove_voice);
                if let Some(code) = paste_from_clipboard() {
                    coop.start_join(code);
                } else {
                    coop.status_text = "Clipboard empty — copy host's ECHO-... code first!".into();
                }
            }
            UiAction::CancelConnecting => {
                clear_remote_players(&mut commands, coop, &mut remove_voice);
                coop.session = None;
                coop.join_code = None;
                coop.pending_disconnect_s = None;
                coop.status_text = "Cancelled — Ready".into();
            }
            UiAction::OpenSettings => {
                save_prompt.open = false;
                device_catalog.refresh();
                settings_open.0 = true;
            }
            UiAction::CloseSettings => {
                settings_open.0 = false;
                audio_dropdown.open = None;
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
                    coop.status_text = "Host a lobby first to get a join code.".into();
                }
            }
            UiAction::DisconnectToMenu => {
                lifecycle_writer.write(SessionLifecycleEvent::DisconnectToMenu);
            }
            UiAction::QuitNoSave => {
                clear_remote_players(&mut commands, coop, &mut remove_voice);
                coop.session = None;
                coop.join_code = None;
                coop.pending_disconnect_s = None;
                exit_writer.write(AppExit::Success);
            }
            UiAction::CycleVisor => {
                let next = (coop.visor_idx + 1) % VISOR_PALETTE.len();
                coop.visor_idx = next;
                user_settings.visor_idx = next;
                if coop.is_connected() {
                    coop.sync_local_profile(inv);
                }
            }
            UiAction::ToggleMute => voice_settings.muted = !voice_settings.muted,
            UiAction::ToggleSelfEcho => {
                voice_settings.self_echo_test = !voice_settings.self_echo_test;
            }
            UiAction::ResumeGame => {
                menu_open.0 = false;
                settings_open.0 = false;
            }
            UiAction::ToggleFullscreen => user_settings.fullscreen = !user_settings.fullscreen,
            UiAction::ToggleIncludePrereleases => {
                user_settings.include_prereleases = !user_settings.include_prereleases;
                let _ = user_settings.save_default();
            }
            UiAction::CheckForUpdates => {
                updater_writer.write(UpdaterCommand::CheckNow);
            }
            UiAction::UpdateInstall => {
                updater_writer.write(UpdaterCommand::StartInstall);
            }
            UiAction::UpdateDismiss => {
                updater_writer.write(UpdaterCommand::DismissPopup);
            }
            UiAction::ToggleAudioDropdown(kind) => {
                if audio_dropdown.open == Some(kind) {
                    audio_dropdown.open = None;
                } else {
                    device_catalog.refresh();
                    audio_dropdown.open = Some(kind);
                    audio_dropdown.page = 0;
                }
            }
            UiAction::SelectAudioRow(kind, row_idx) => {
                let abs_idx = audio_dropdown.page * AUDIO_DROPDOWN_ROWS + row_idx;
                let list = match kind {
                    AudioDropdownKind::Output => &device_catalog.output_devices,
                    AudioDropdownKind::Input => &device_catalog.input_devices,
                };
                if abs_idx == 0 {
                    match kind {
                        AudioDropdownKind::Output => user_settings.output_device = None,
                        AudioDropdownKind::Input => user_settings.input_device = None,
                    }
                } else if let Some(dev_name) = list.get(abs_idx - 1).cloned() {
                    match kind {
                        AudioDropdownKind::Output => user_settings.output_device = Some(dev_name),
                        AudioDropdownKind::Input => user_settings.input_device = Some(dev_name),
                    }
                }
                audio_dropdown.open = None;
            }
            UiAction::AudioListPage(delta) => {
                if let Some(kind) = audio_dropdown.open {
                    let count = match kind {
                        AudioDropdownKind::Output => device_catalog.output_devices.len() + 1,
                        AudioDropdownKind::Input => device_catalog.input_devices.len() + 1,
                    };
                    let total_pages = count.div_ceil(AUDIO_DROPDOWN_ROWS).max(1);
                    let next = (audio_dropdown.page as i32 + delta).rem_euclid(total_pages as i32);
                    audio_dropdown.page = next as usize;
                }
            }
            UiAction::RefreshAudioDevices => {
                device_catalog.refresh();
                audio_dropdown.page = 0;
            }
            UiAction::SliderStep(kind, dir) => apply_slider_step(user_settings, kind, dir),
            UiAction::SliderSetFrac(kind, frac) => apply_slider_frac(user_settings, kind, frac),
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

// =============================================================================
// PANEL VISIBILITY (Single Unified Query — Zero B0001 Risk!)
// =============================================================================

#[allow(clippy::too_many_arguments)]
fn update_panel_visibility_system(
    screen: Res<AppScreen>,
    menu_open: Res<CoopMenuOpen>,
    shop_open: Res<ShopOverlayOpen>,
    settings_open: Res<SettingsMenuOpen>,
    save_prompt: Res<SaveNamePromptState>,
    parkour_timer: Res<ParkourTimerState>,
    diag_open: Res<DiagnosticsDrawerOpen>,
    updater: Res<UpdaterState>,
    inv: Res<PlayerInventory>,
    target: Res<CrosshairTargetState>,
    toast: Res<GameplayToast>,
    mut panels_q: Query<(&UiPanelKind, &mut Visibility, &mut Node)>,
) {
    let in_game = *screen == AppScreen::InGame;
    let gameplay_clear = in_game && !menu_open.0 && !shop_open.0 && !settings_open.0;

    for (kind, mut v, mut node) in &mut panels_q {
        let show = match kind {
            UiPanelKind::MainMenu => !in_game && !settings_open.0,
            UiPanelKind::Settings => settings_open.0,
            UiPanelKind::SaveNameModal => !in_game && !settings_open.0 && save_prompt.open,
            UiPanelKind::UpdatePopup => !in_game && updater.popup_open,
            UiPanelKind::InGameHud => in_game,
            UiPanelKind::CoopMenu => in_game && menu_open.0 && !settings_open.0,
            UiPanelKind::ShopOverlay => in_game && shop_open.0 && !settings_open.0,
            UiPanelKind::Crosshair => gameplay_clear,
            UiPanelKind::TargetCard => gameplay_clear && !target.label.is_empty(),
            UiPanelKind::ToastCard => gameplay_clear && !toast.text.is_empty(),
            UiPanelKind::BatteryTray => gameplay_clear && inv.battery_selector.open,
            UiPanelKind::ParkourHud => gameplay_clear && parkour_timer.in_room,
            UiPanelKind::DiagDrawer => in_game && diag_open.0,
        };
        *v = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if matches!(
            kind,
            UiPanelKind::MainMenu
                | UiPanelKind::Settings
                | UiPanelKind::SaveNameModal
                | UiPanelKind::UpdatePopup
                | UiPanelKind::CoopMenu
                | UiPanelKind::ShopOverlay
        ) {
            node.display = if show { Display::Flex } else { Display::None };
        }
    }
}

// =============================================================================
// MAIN MENU, PAUSE MENU & SUPPLY SHOP VISUAL UPDATES
// =============================================================================

#[derive(SystemParam)]
struct MenuShopRead<'w> {
    coop: Res<'w, CoopState>,
    user_settings: Res<'w, UserSettings>,
    name_focused: Res<'w, NameInputFocused>,
    campaign: Res<'w, CampaignState>,
    save_browser: Res<'w, SaveSlotBrowser>,
}

#[allow(clippy::type_complexity)]
fn update_menu_and_shop_system(
    time: Res<Time>,
    st: MenuShopRead,
    mut texts: ParamSet<(
        Query<&mut Text, With<MainMenuNameText>>,
        Query<&mut Text, With<MainMenuStatusText>>,
        Query<&mut Text, With<MainMenuSaveNameText>>,
        Query<&mut Text, With<MainMenuSaveMetaText>>,
        Query<&mut Text, With<CoopPanelStatusText>>,
        Query<(&mut Text, &mut TextColor), With<ShopCreditsText>>,
        Query<&mut Text, With<ShopSaveTagText>>,
    )>,
    mut borders: ParamSet<(
        Query<(&mut BorderColor, &mut BackgroundColor), With<MainMenuNameBox>>,
        Query<(&VisorHelmetButton, &mut BorderColor)>,
        Query<(&ShopTierTab, &mut BorderColor, &mut BackgroundColor)>,
        Query<(&SaveTierPip, &mut BackgroundColor)>,
    )>,
) {
    let coop = &st.coop;
    let user_settings = &st.user_settings;
    let name_focused = &st.name_focused;
    let campaign = &st.campaign;
    let save_browser = &st.save_browser;

    let blink_on = ((time.elapsed_secs() / 0.50).floor() as u64).is_multiple_of(2);

    if let Ok(mut t) = texts.p0().single_mut() {
        let cursor = if name_focused.0 {
            if blink_on { "|" } else { " " }
        } else {
            ""
        };
        t.0 = format!("{}{}", user_settings.player_name, cursor);
    }
    if let Ok(mut t) = texts.p1().single_mut() {
        t.0 = coop.status_text.clone();
    }

    let slot_unlocked_tier = save_browser
        .selected()
        .map_or(1, |s| s.unlocked_depth_tier.clamp(1, 3));
    if let Ok(mut t) = texts.p2().single_mut() {
        let total = save_browser.slots.len().max(1);
        let idx = (save_browser.selected_idx + 1).min(total);
        t.0 = format!("{}  ({}/{})", save_browser.selected_slot_name(), idx, total);
    }
    if let Ok(mut t) = texts.p3().single_mut() {
        t.0 = if let Some(slot) = save_browser.selected() {
            if slot.exists_on_disk {
                format!(
                    "{} cr  •  Tier {}  •  {} dive(s)  •  {} miner(s)",
                    slot.credits,
                    slot.unlocked_depth_tier,
                    slot.dives_completed,
                    slot.player_count
                )
            } else {
                format!("New Campaign  •  {} cr", slot.credits)
            }
        } else {
            "New Campaign".to_string()
        };
    }

    for (pip, mut bg) in &mut borders.p3() {
        *bg = if pip.0 <= slot_unlocked_tier {
            BackgroundColor(COL_ACCENT_CYAN)
        } else {
            BackgroundColor(Color::srgba(0.16, 0.21, 0.27, 0.9))
        };
    }

    if let Ok(mut t) = texts.p4().single_mut() {
        let code_preview = match &coop.join_code {
            Some(c) if c.len() > 38 => format!("{}...{}", &c[..20], &c[c.len() - 10..]),
            Some(c) => c.clone(),
            None => "Offline Solo".into(),
        };
        let peers: Vec<String> = coop
            .remote_players
            .values()
            .map(|rp| rp.info.name.clone())
            .collect();
        let peer_str = if peers.is_empty() {
            "Solo".to_string()
        } else {
            peers.join(", ")
        };
        t.0 = format!(
            "Save: {}   •   Code: {}\nMiner: {} ({})   •   Crew: {}",
            campaign.slot_name,
            code_preview,
            coop.local_name,
            coop.visor_label(),
            peer_str
        );
    }

    if let Ok((mut t, mut col)) = texts.p5().single_mut() {
        let cr = campaign.save.credits;
        t.0 = if cr < 0 {
            format!("{cr} cr (DEBT)")
        } else {
            format!("{cr} cr")
        };
        *col = TextColor(if cr <= 0 {
            COL_ACCENT_RED
        } else {
            COL_ACCENT_AMBER
        });
    }
    if let Ok(mut t) = texts.p6().single_mut() {
        let debt_note = if campaign.save.credits <= 0 {
            "  •  DEBT: SHOP LOCKED"
        } else {
            ""
        };
        t.0 = format!(
            "Save: {}  •  Dives: {}  •  Unlocked: Tier {}{}",
            campaign.slot_name,
            campaign.save.dives_completed,
            campaign.save.unlocked_depth_tier,
            debt_note
        );
    }

    if let Ok((mut b, mut bg)) = borders.p0().single_mut() {
        *b = BorderColor::all(if name_focused.0 {
            COL_ACCENT_CYAN
        } else {
            COL_BORDER_DIM
        });
        *bg = BackgroundColor(if name_focused.0 {
            Color::srgba(0.06, 0.14, 0.20, 0.98)
        } else {
            Color::srgba(0.05, 0.08, 0.12, 0.98)
        });
    }
    let active_visor = coop.visor_idx % VISOR_PALETTE.len();
    for (btn, mut b) in &mut borders.p1() {
        *b = BorderColor::all(if btn.0 == active_visor {
            COL_ACCENT_CYAN
        } else {
            COL_BORDER_DIM
        });
    }
    for (tab, mut b, mut bg) in &mut borders.p2() {
        let selected = tab.0 == campaign.save.selected_depth_tier;
        let unlocked = tab.0 <= campaign.save.unlocked_depth_tier;
        if selected {
            *b = BorderColor::all(COL_ACCENT_CYAN);
            *bg = BackgroundColor(Color::srgba(0.10, 0.28, 0.30, 0.96));
        } else if unlocked {
            *b = BorderColor::all(COL_BORDER_DIM);
            *bg = BackgroundColor(COL_CARD_BG);
        } else {
            *b = BorderColor::all(Color::srgba(0.16, 0.18, 0.22, 0.55));
            *bg = BackgroundColor(Color::srgba(0.05, 0.06, 0.08, 0.72));
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_save_dropdown_system(
    save_browser: Res<SaveSlotBrowser>,
    mut dropdown: ResMut<SaveDropdownState>,
    mut nodes: ParamSet<(
        Query<&mut Node, With<SaveDropdownList>>,
        Query<(&SaveDropdownRow, &mut Node, &mut BackgroundColor, &mut BorderColor)>,
        Query<&mut Node, With<SaveDropdownPagerRow>>,
    )>,
    mut texts: ParamSet<(
        Query<&mut Text, With<SaveDropdownChevron>>,
        Query<(&SaveDropdownRowName, &mut Text, &mut TextColor)>,
        Query<(&SaveDropdownRowMeta, &mut Text)>,
        Query<&mut Text, With<SaveDropdownPageLabel>>,
    )>,
) {
    let total_pages = save_browser.slots.len().div_ceil(SAVE_DROPDOWN_ROWS).max(1);
    if dropdown.page >= total_pages {
        dropdown.page = total_pages - 1;
    }

    if let Ok(mut ch) = texts.p0().single_mut() {
        ch.0 = if dropdown.open { "▲" } else { "▼" }.to_string();
    }
    if let Ok(mut list_node) = nodes.p0().single_mut() {
        list_node.display = if dropdown.open {
            Display::Flex
        } else {
            Display::None
        };
    }
    if !dropdown.open {
        return;
    }

    let start = dropdown.page * SAVE_DROPDOWN_ROWS;
    for (row, mut node, mut bg, mut border) in &mut nodes.p1() {
        let abs_idx = start + row.idx;
        if abs_idx < save_browser.slots.len() {
            node.display = Display::Flex;
            let selected = abs_idx == save_browser.selected_idx;
            *bg = BackgroundColor(if selected && row.hovered {
                Color::srgba(0.13, 0.34, 0.38, 0.98)
            } else if selected {
                Color::srgba(0.09, 0.24, 0.28, 0.98)
            } else if row.hovered {
                Color::srgba(0.13, 0.21, 0.29, 0.98)
            } else {
                COL_CARD_BG
            });
            *border = BorderColor::all(if selected || row.hovered {
                COL_ACCENT_CYAN
            } else {
                COL_BORDER_DIM
            });
        } else {
            node.display = Display::None;
        }
    }
    for (rn, mut t, mut col) in &mut texts.p1() {
        let abs_idx = start + rn.0;
        if let Some(slot) = save_browser.slots.get(abs_idx) {
            t.0 = slot.slot_name.clone();
            *col = TextColor(if abs_idx == save_browser.selected_idx {
                COL_ACCENT_CYAN
            } else {
                COL_TEXT_BRIGHT
            });
        }
    }
    for (rm, mut t) in &mut texts.p2() {
        let abs_idx = start + rm.0;
        if let Some(slot) = save_browser.slots.get(abs_idx) {
            t.0 = if slot.exists_on_disk {
                format!(
                    "{} cr • T{} • {}d",
                    slot.credits, slot.unlocked_depth_tier, slot.dives_completed
                )
            } else {
                format!("New • {} cr", slot.credits)
            };
        }
    }
    if let Ok(mut pager) = nodes.p2().single_mut() {
        pager.display = if total_pages > 1 {
            Display::Flex
        } else {
            Display::None
        };
    }
    if let Ok(mut lbl) = texts.p3().single_mut() {
        lbl.0 = format!("Page {} / {}", dropdown.page + 1, total_pages);
    }
}

#[allow(clippy::type_complexity)]
fn update_save_name_modal_system(
    time: Res<Time>,
    prompt: Res<SaveNamePromptState>,
    mut box_q: Query<(&mut BorderColor, &mut BackgroundColor), With<SaveNameInputBox>>,
    mut texts: ParamSet<(
        Query<&mut Text, With<SaveNameModalTitle>>,
        Query<&mut Text, With<SaveNameModalSubtitle>>,
        Query<(&mut Text, &mut TextColor), With<SaveNameInputText>>,
        Query<(&mut Text, &mut TextColor), With<SaveNameHintText>>,
        Query<&mut Text, With<SaveNameConfirmLabel>>,
        Query<(&mut Text, &mut TextColor), With<SaveNameFocusBadge>>,
    )>,
) {
    if !prompt.open {
        return;
    }

    let is_rename = prompt.mode == Some(SaveNamePromptMode::Rename);
    let blink_elapsed = (time.elapsed_secs() - prompt.caret_reset_s).max(0.0);
    let caret_visible = ((blink_elapsed / 0.50).floor() as u64).is_multiple_of(2);

    if let Ok((mut border, mut bg)) = box_q.single_mut() {
        if prompt.focused {
            *border = BorderColor::all(COL_ACCENT_CYAN);
            *bg = BackgroundColor(Color::srgba(0.06, 0.14, 0.20, 0.98));
        } else {
            *border = BorderColor::all(COL_BORDER_DIM);
            *bg = BackgroundColor(Color::srgba(0.04, 0.06, 0.09, 0.96));
        }
    }

    if let Ok(mut t) = texts.p0().single_mut() {
        t.0 = if is_rename {
            "Rename Save".to_string()
        } else {
            "New Save".to_string()
        };
    }

    if let Ok(mut t) = texts.p1().single_mut() {
        t.0 = if is_rename {
            format!("Rename '{}' to a new name:", prompt.target_old_name)
        } else {
            "Choose a name for your new expedition save:".to_string()
        };
    }

    if let Ok((mut t, mut col)) = texts.p2().single_mut() {
        let caret = if prompt.focused {
            if caret_visible { "|" } else { " " }
        } else {
            ""
        };
        if prompt.buffer.is_empty() && !is_rename {
            if prompt.focused {
                t.0 = format!("{caret} Type name... (Enter for '{}')", prompt.default_hint);
            } else {
                t.0 = format!("Click here to type... (Enter for '{}')", prompt.default_hint);
            }
            *col = TextColor(COL_TEXT_MUTED);
        } else {
            t.0 = format!("{}{caret}", prompt.buffer);
            *col = TextColor(if prompt.focused {
                COL_TEXT_BRIGHT
            } else {
                COL_TEXT_MUTED
            });
        }
    }

    if let Ok((mut t, mut col)) = texts.p3().single_mut() {
        if !prompt.error_text.is_empty() {
            t.0 = prompt.error_text.clone();
            *col = TextColor(COL_ACCENT_RED);
        } else {
            let preview = if prompt.buffer.trim().is_empty() && !is_rename {
                Some(prompt.default_hint.clone())
            } else {
                validate_save_slot_name(&prompt.buffer).ok()
            };
            t.0 = match preview {
                Some(stem) => format!("File: saves/{stem}.json  •  Enter confirm  •  Del clear"),
                None => "Letters, numbers, '-' or '_'  •  Esc to cancel".to_string(),
            };
            *col = TextColor(COL_TEXT_MUTED);
        }
    }

    if let Ok(mut t) = texts.p4().single_mut() {
        t.0 = if is_rename {
            "Rename Save".to_string()
        } else {
            "Create Save".to_string()
        };
    }

    if let Ok((mut t, mut col)) = texts.p5().single_mut() {
        if prompt.focused {
            t.0 = "TYPING".to_string();
            *col = TextColor(COL_ACCENT_CYAN);
        } else {
            t.0 = "CLICK TO TYPE".to_string();
            *col = TextColor(COL_TEXT_MUTED);
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_parkour_hud_system(
    parkour: Res<ParkourTimerState>,
    campaign: Res<CampaignState>,
    mut texts: ParamSet<(
        Query<(&mut Text, &mut TextColor), With<ParkourClockText>>,
        Query<(&mut Text, &mut TextColor), With<ParkourClockStatusText>>,
        Query<(&ParkourBoardRow, &mut Text, &mut TextColor, &mut Node)>,
    )>,
) {
    if !parkour.in_room {
        return;
    }

    if let Ok((mut t, mut col)) = texts.p0().single_mut() {
        if parkour.running {
            let ms = (parkour.elapsed_s * 1000.0).round() as u32;
            t.0 = format_parkour_time_ms(ms);
            *col = TextColor(COL_ACCENT_CYAN);
        } else if let Some(finish_ms) = parkour.last_finish_ms
            && parkour.finish_banner_s > 0.0
        {
            t.0 = format_parkour_time_ms(finish_ms);
            *col = TextColor(COL_ACCENT_GREEN);
        } else {
            t.0 = "0.00s".to_string();
            *col = TextColor(COL_TEXT_MUTED);
        }
    }

    if let Ok((mut t, mut col)) = texts.p1().single_mut() {
        if parkour.running {
            t.0 = "RUNNING • Click Summit Bell [E]".to_string();
            *col = TextColor(COL_ACCENT_CYAN);
        } else if let Some(rank) = parkour.last_finish_rank
            && parkour.finish_banner_s > 0.0
        {
            t.0 = if rank == 1 {
                "NEW #1 RECORD!".to_string()
            } else {
                format!("FINISHED • Placed #{rank}")
            };
            *col = TextColor(COL_ACCENT_GREEN);
        } else if parkour.last_finish_ms.is_some() && parkour.finish_banner_s > 0.0 {
            t.0 = "FINISHED!".to_string();
            *col = TextColor(COL_ACCENT_GREEN);
        } else {
            t.0 = "Pass emerald Start Gate to begin".to_string();
            *col = TextColor(COL_TEXT_MUTED);
        }
    }

    let board = &campaign.save.parkour_leaderboard;
    for (row, mut t, mut col, mut node) in &mut texts.p2() {
        if board.is_empty() {
            if row.0 == 0 {
                node.display = Display::Flex;
                t.0 = "No official runs yet".to_string();
                *col = TextColor(COL_TEXT_MUTED);
            } else {
                node.display = Display::None;
            }
        } else if let Some(rec) = board.get(row.0) {
            node.display = Display::Flex;
            let time_str = format_parkour_time_ms(rec.time_ms);
            t.0 = format!("#{}  {}  —  {time_str}", row.0 + 1, rec.player_name);
            *col = TextColor(match row.0 {
                0 => COL_ACCENT_AMBER,
                1 => COL_TEXT_BRIGHT,
                2 => COL_ACCENT_CYAN,
                _ => COL_TEXT_MUTED,
            });
        } else {
            node.display = Display::None;
        }
    }
}

// =============================================================================
// IN-GAME GRAPHICAL BARS, LED MIC METER & DYNAMIC CROSSHAIR
// =============================================================================

#[derive(SystemParam)]
struct InGameBarsRead<'w> {
    telemetry: Res<'w, AcousticTelemetry>,
    voice_settings: Res<'w, VoiceSettings>,
    elevator: Res<'w, ElevatorState>,
    campaign: Res<'w, CampaignState>,
    health: Res<'w, PlayerHealthState>,
    toast: Res<'w, GameplayToast>,
}

#[allow(clippy::type_complexity)]
fn update_ingame_bars_system(
    st: InGameBarsRead,
    fl_q: Query<&PlayerFlashlight>,
    mut texts: ParamSet<(
        Query<&mut Text, With<HudTierBadgeText>>,
        Query<(&mut Text, &mut TextColor), With<HudCreditsText>>,
        Query<&mut Text, With<HudPhaseText>>,
        Query<&mut Text, With<HudHealthBadgeText>>,
        Query<&mut Text, With<ToastText>>,
    )>,
    mut bars: ParamSet<(
        Query<(&mut Node, &mut BackgroundColor), With<HudElevatorProgressFill>>,
        Query<(&mut Node, &mut BackgroundColor), With<HudHealthFill>>,
        Query<(&mut Node, &mut BackgroundColor), With<HudBatteryFill>>,
        Query<(&HudMicLed, &mut BackgroundColor)>,
        Query<&mut BackgroundColor, With<HudMicStateDot>>,
        Query<&mut BackgroundColor, With<HudBatteryLampDot>>,
        Query<&mut BackgroundColor, With<DamageFlashOverlay>>,
    )>,
) {
    let campaign = &st.campaign;
    let elevator = &st.elevator;
    let health = &st.health;
    let toast = &st.toast;
    let mic = st.telemetry.mic;
    let voice_settings = &st.voice_settings;

    if let Ok(mut t) = texts.p0().single_mut() {
        t.0 = format!("T{}", campaign.save.selected_depth_tier);
    }
    if let Ok((mut t, mut col)) = texts.p1().single_mut() {
        let cr = campaign.save.credits;
        t.0 = format!("{cr} cr");
        *col = TextColor(if cr < 0 {
            COL_ACCENT_RED
        } else {
            COL_ACCENT_AMBER
        });
    }

    let (phase_label, phase_frac, phase_col) = match elevator.phase {
        ElevatorPhase::ShedIdle => ("SHED".to_string(), 0.0, COL_ACCENT_CYAN),
        ElevatorPhase::Descending => (
            "DESCEND".to_string(),
            (elevator.phase_timer_s / ELEVATOR_DESCENT_DURATION_S).clamp(0.0, 1.0),
            COL_ACCENT_CYAN,
        ),
        ElevatorPhase::CaveIdle => ("CAVE".to_string(), 0.0, COL_TEXT_MUTED),
        ElevatorPhase::WinchActive => {
            let rem = (ELEVATOR_WINCH_DURATION_S - elevator.phase_timer_s).max(0.0);
            (
                format!("WINCH {:.0}s", rem.ceil()),
                (elevator.phase_timer_s / ELEVATOR_WINCH_DURATION_S).clamp(0.0, 1.0),
                COL_ACCENT_RED,
            )
        }
        ElevatorPhase::Ascending => (
            "EXTRACT".to_string(),
            (elevator.phase_timer_s / ELEVATOR_ASCENT_DURATION_S).clamp(0.0, 1.0),
            COL_ACCENT_GREEN,
        ),
    };
    if let Ok(mut t) = texts.p2().single_mut() {
        t.0 = phase_label;
    }
    if let Ok((mut node, mut bg)) = bars.p0().single_mut() {
        node.width = Val::Percent(phase_frac * 100.0);
        *bg = BackgroundColor(phase_col);
    }

    // Health bar fill + tag
    let (hp_frac, hp_col, hp_tag) = match health.phase {
        LocalHealthPhase::Healthy => (1.0f32, COL_ACCENT_GREEN, "OK".to_string()),
        LocalHealthPhase::Downed {
            bleedout_s,
            dragged_by,
        } => (
            (bleedout_s / BLEEDOUT_DURATION_S).clamp(0.0, 1.0),
            COL_ACCENT_RED,
            if dragged_by.is_some() {
                format!("CARRIED {:.0}s", bleedout_s.ceil())
            } else {
                format!("{:.0}s", bleedout_s.ceil())
            },
        ),
        LocalHealthPhase::Dead => (0.0f32, Color::srgb(0.4, 0.42, 0.46), "DOWN".to_string()),
    };
    if let Ok(mut t) = texts.p3().single_mut() {
        t.0 = hp_tag;
    }
    if let Ok((mut node, mut bg)) = bars.p1().single_mut() {
        node.width = Val::Percent(hp_frac * 100.0);
        *bg = BackgroundColor(hp_col);
    }
    if let Ok(mut t) = texts.p4().single_mut() {
        t.0 = toast.text.clone();
    }

    // Flashlight / Headlamp Battery Cell Graphic
    let (bat_frac, fl_on) = fl_q
        .single()
        .map(|f| (f.battery.clamp(0.0, 1.0), f.active_beam))
        .unwrap_or((0.0, false));
    let bat_col = if bat_frac > 0.45 {
        COL_ACCENT_CYAN
    } else if bat_frac > 0.20 {
        COL_ACCENT_AMBER
    } else {
        COL_ACCENT_RED
    };
    if let Ok((mut node, mut bg)) = bars.p2().single_mut() {
        node.width = Val::Percent(bat_frac * 100.0);
        *bg = BackgroundColor(bat_col);
    }
    if let Ok(mut dot) = bars.p5().single_mut() {
        *dot = BackgroundColor(if fl_on && bat_frac > 0.01 {
            COL_ACCENT_AMBER
        } else {
            Color::srgba(0.20, 0.24, 0.30, 0.85)
        });
    }

    // 10-Bar LED Mic Meter
    let norm_mic = ((mic.rms_db + 68.0) / 58.0).clamp(0.0, 1.0);
    let lit_bars = if !mic.available || voice_settings.muted {
        0
    } else {
        (norm_mic * MIC_LED_BARS as f32).round() as usize
    };
    for (led, mut bg) in &mut bars.p3() {
        *bg = if led.0 < lit_bars {
            if !mic.gate_open {
                BackgroundColor(Color::srgba(0.28, 0.42, 0.52, 0.9))
            } else if led.0 >= 7 || mic.ai_noise_level >= 1.2 {
                BackgroundColor(COL_ACCENT_RED)
            } else if led.0 >= 4 || mic.ai_noise_level >= 0.35 {
                BackgroundColor(COL_ACCENT_AMBER)
            } else {
                BackgroundColor(COL_ACCENT_GREEN)
            }
        } else {
            BackgroundColor(Color::srgba(0.13, 0.18, 0.24, 0.78))
        };
    }
    if let Ok(mut dot) = bars.p4().single_mut() {
        *dot = BackgroundColor(if !mic.available {
            Color::srgb(0.35, 0.38, 0.42)
        } else if voice_settings.muted || (mic.gate_open && mic.ai_noise_level >= 1.2) {
            COL_ACCENT_RED
        } else if mic.gate_open {
            COL_ACCENT_GREEN
        } else {
            Color::srgb(0.25, 0.46, 0.52)
        });
    }

    // Damage vignette
    if let Ok(mut bg) = bars.p6().single_mut() {
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
fn update_crosshair_and_target_system(
    target: Res<CrosshairTargetState>,
    mut texts: ParamSet<(
        Query<&mut Text, With<TargetKeyBadgeText>>,
        Query<&mut Text, With<TargetLabelText>>,
        Query<&mut Text, With<TargetSublabelText>>,
    )>,
    mut colors: ParamSet<(
        Query<&mut BackgroundColor, With<CrosshairCenterDot>>,
        Query<&mut BackgroundColor, With<CrosshairBracket>>,
        Query<&mut BackgroundColor, With<TargetKeyBadgeBg>>,
    )>,
    mut target_border_q: Query<&mut BorderColor, With<TargetCardFrame>>,
) {
    if let Ok(mut t) = texts.p0().single_mut() {
        t.0 = target.key_badge.clone();
    }
    if let Ok(mut t) = texts.p1().single_mut() {
        t.0 = target.label.clone();
    }
    if let Ok(mut t) = texts.p2().single_mut() {
        t.0 = target.sublabel.clone();
    }

    let accent_col = match target.accent {
        TargetAccent::None => Color::srgba(0.95, 0.97, 1.0, 0.85),
        TargetAccent::Interact => COL_ACCENT_CYAN,
        TargetAccent::Danger => COL_ACCENT_RED,
        TargetAccent::Blocked => COL_ACCENT_AMBER,
    };
    if let Ok(mut b) = target_border_q.single_mut() {
        *b = BorderColor::all(accent_col);
    }
    if let Ok(mut dot) = colors.p0().single_mut() {
        *dot = BackgroundColor(accent_col);
    }
    let bracket_col = if target.accent == TargetAccent::None {
        Color::NONE
    } else {
        accent_col
    };
    for mut br in &mut colors.p1() {
        *br = BackgroundColor(bracket_col);
    }
    if let Ok(mut key_bg) = colors.p2().single_mut() {
        *key_bg = BackgroundColor(accent_col);
    }
}

// =============================================================================
// GRAPHICAL HOTBAR, BATTERY POUCH GAUGES & WEARABLES
// =============================================================================

fn belt_item_graphics(item: Option<&BeltItem>) -> (Color, Color, String) {
    match item {
        None => (Color::srgba(0.22, 0.28, 0.34, 0.35), Color::NONE, String::new()),
        Some(BeltItem::Flashlight) => (
            Color::srgb(0.32, 0.38, 0.46),
            Color::srgb(1.0, 0.92, 0.45),
            "FL".to_string(),
        ),
        Some(BeltItem::Flares { count }) => (
            Color::srgb(0.92, 0.26, 0.16),
            Color::srgb(1.0, 0.78, 0.32),
            format!("x{count}"),
        ),
        Some(BeltItem::AcousticFoam { charges }) => (
            Color::srgb(0.20, 0.78, 0.66),
            Color::srgb(0.86, 0.98, 0.95),
            format!("x{charges}"),
        ),
        Some(BeltItem::NoiseDecoys { count }) => (
            Color::srgb(0.96, 0.70, 0.16),
            Color::srgb(0.16, 0.12, 0.05),
            format!("x{count}"),
        ),
        Some(BeltItem::AidKit) => (
            Color::srgb(0.94, 0.96, 0.98),
            Color::srgb(0.94, 0.18, 0.22),
            "+".to_string(),
        ),
        Some(BeltItem::BonusLoot { value }) => (
            Color::srgb(0.24, 0.90, 0.98),
            Color::srgb(1.0, 0.88, 0.38),
            format!("{value}"),
        ),
    }
}

#[allow(clippy::type_complexity)]
fn update_hotbar_and_pouch_system(
    inv: Res<PlayerInventory>,
    items_q: Query<&WorldItemEntity>,
    mut visuals: ParamSet<(
        Query<(&HotbarSlotCard, &mut BorderColor, &mut BackgroundColor)>,
        Query<(&HotbarIconPrimary, &mut BackgroundColor)>,
        Query<(&HotbarIconSecondary, &mut BackgroundColor)>,
        Query<(&HotbarChargeFill, &mut Node, &mut BackgroundColor)>,
        Query<(&PouchGaugeFill, &mut Node, &mut BackgroundColor)>,
        Query<(&WearableBadge, &mut BorderColor, &mut BackgroundColor)>,
    )>,
    mut texts: ParamSet<(
        Query<(&HotbarCountText, &mut Text)>,
        Query<&mut Text, With<HotbarActiveNameText>>,
        Query<&mut Text, With<HeadlampBadgeText>>,
    )>,
) {
    let loaded_bat = inv.loaded_battery_charge().clamp(0.0, 1.0);

    for (card, mut border, mut bg) in &mut visuals.p0() {
        let idx = card.0;
        let unlocked = idx < inv.belt_capacity;
        let selected = unlocked && idx == inv.active_slot && !inv.is_hands_occupied();
        if !unlocked {
            *border = BorderColor::all(Color::srgba(0.15, 0.18, 0.22, 0.45));
            *bg = BackgroundColor(Color::srgba(0.03, 0.04, 0.06, 0.55));
        } else if selected {
            *border = BorderColor::all(COL_ACCENT_CYAN);
            *bg = BackgroundColor(Color::srgba(0.09, 0.20, 0.26, 0.95));
        } else {
            *border = BorderColor::all(COL_BORDER_DIM);
            *bg = BackgroundColor(COL_HUD_POD_BG);
        }
    }

    for (ic, mut bg) in &mut visuals.p1() {
        let unlocked = ic.0 < inv.belt_capacity;
        let item = if unlocked {
            inv.belt.get(ic.0).and_then(|s| s.as_ref())
        } else {
            None
        };
        let (prim, _, _) = belt_item_graphics(item);
        *bg = BackgroundColor(if unlocked { prim } else { Color::NONE });
    }
    for (ic, mut bg) in &mut visuals.p2() {
        let unlocked = ic.0 < inv.belt_capacity;
        let item = if unlocked {
            inv.belt.get(ic.0).and_then(|s| s.as_ref())
        } else {
            None
        };
        let (_, sec, _) = belt_item_graphics(item);
        *bg = BackgroundColor(if unlocked { sec } else { Color::NONE });
    }
    for (ct, mut t) in &mut texts.p0() {
        let unlocked = ct.0 < inv.belt_capacity;
        let item = if unlocked {
            inv.belt.get(ct.0).and_then(|s| s.as_ref())
        } else {
            None
        };
        let (_, _, label) = belt_item_graphics(item);
        t.0 = if unlocked { label } else { "LOCK".to_string() };
    }
    if let Ok(mut name_t) = texts.p1().single_mut() {
        name_t.0 = match inv.carry_state {
            CarryHandState::HeavyRelic { item_id } => items_q
                .iter()
                .find(|item| item.id == item_id)
                .map(|item| world_item_display_name(&item.kind))
                .unwrap_or_else(|| "Heavy Relic".to_string()),
            CarryHandState::DraggingPlayer { .. } => "Carrying Companion".to_string(),
            CarryHandState::Free => match inv.active_item() {
                None => String::new(),
                Some(BeltItem::Flashlight) => {
                    format!("Flashlight ({:.0}%)", loaded_bat * 100.0)
                }
                Some(BeltItem::Flares { count }) => format!("Glow Flares ({count})"),
                Some(BeltItem::AcousticFoam { charges }) => {
                    format!("Acoustic Foam ({charges})")
                }
                Some(BeltItem::NoiseDecoys { count }) => format!("Noise Decoys ({count})"),
                Some(BeltItem::AidKit) => "Aid Kit".to_string(),
                Some(BeltItem::BonusLoot { value }) => format!("Bonus Crystal ({value} cr)"),
            },
        };
    }
    for (fill, mut node, mut bg) in &mut visuals.p3() {
        let is_fl = fill.0 < inv.belt_capacity
            && matches!(
                inv.belt.get(fill.0).and_then(|s| s.as_ref()),
                Some(BeltItem::Flashlight)
            );
        if is_fl {
            node.width = Val::Percent(loaded_bat * 100.0);
            *bg = BackgroundColor(if loaded_bat > 0.25 {
                COL_ACCENT_CYAN
            } else {
                COL_ACCENT_RED
            });
        } else {
            node.width = Val::Percent(0.0);
        }
    }

    // Mini Battery Pouch vertical gauges (first 4 pouch cells)
    for (g, mut node, mut bg) in &mut visuals.p4() {
        if let Some(cell) = inv.battery_pouch.get(g.0) {
            let pct = cell.charge.clamp(0.0, 1.0);
            node.height = Val::Percent((pct * 100.0).max(8.0));
            *bg = BackgroundColor(if pct > 0.45 {
                COL_ACCENT_CYAN
            } else if pct > 0.15 {
                COL_ACCENT_AMBER
            } else {
                COL_ACCENT_RED
            });
        } else {
            node.height = Val::Percent(0.0);
        }
    }

    // Passive Wearable Badges (with live Headlamp sealed-battery % readout!)
    if let Ok(mut hl_txt) = texts.p2().single_mut() {
        hl_txt.0 = if inv.passive.headlamp {
            format!("[F] {}%", inv.headlamp_charge_pct())
        } else {
            "HL".to_string()
        };
    }
    for (wb, mut border, mut bg) in &mut visuals.p5() {
        let owned = if wb.is_headlamp {
            inv.passive.headlamp
        } else {
            inv.passive.stealth_boots
        };
        if owned {
            let col = if wb.is_headlamp {
                if inv.headlamp_charge <= 0.01 {
                    COL_ACCENT_RED
                } else if inv.headlamp_charge <= 0.25 {
                    COL_ACCENT_AMBER
                } else {
                    COL_ACCENT_CYAN
                }
            } else {
                Color::srgb(0.60, 0.46, 1.0)
            };
            *border = BorderColor::all(col);
            *bg = BackgroundColor(Color::srgba(0.14, 0.20, 0.28, 0.96));
        } else {
            *border = BorderColor::all(Color::srgba(0.18, 0.22, 0.28, 0.55));
            *bg = BackgroundColor(Color::srgba(0.05, 0.06, 0.09, 0.60));
        }
    }
}

// =============================================================================
// GRAPHICAL [R] BATTERY SWAP TRAY
// =============================================================================

#[allow(clippy::type_complexity)]
fn update_battery_tray_system(
    inv: Res<PlayerInventory>,
    mut cards_q: Query<(&TrayCellCard, &mut BorderColor, &mut BackgroundColor)>,
    mut fills_q: Query<(&TrayCellFill, &mut Node, &mut BackgroundColor), Without<TrayCellCard>>,
    mut pcts_q: Query<(&TrayCellPctText, &mut Text)>,
) {
    if !inv.battery_selector.open {
        return;
    }

    let sel_idx = inv.battery_selector.selected_idx;
    for (card, mut border, mut bg) in &mut cards_q {
        let present = card.0 < inv.battery_pouch.len();
        let selected = present && card.0 == sel_idx;
        if selected {
            *border = BorderColor::all(COL_ACCENT_CYAN);
            *bg = BackgroundColor(Color::srgba(0.11, 0.26, 0.32, 0.98));
        } else if present {
            *border = BorderColor::all(COL_BORDER_DIM);
            *bg = BackgroundColor(COL_CARD_BG);
        } else {
            *border = BorderColor::all(Color::srgba(0.14, 0.18, 0.22, 0.45));
            *bg = BackgroundColor(Color::srgba(0.04, 0.05, 0.08, 0.55));
        }
    }
    for (fill, mut node, mut bg) in &mut fills_q {
        if let Some(cell) = inv.battery_pouch.get(fill.0) {
            let pct = cell.charge.clamp(0.0, 1.0);
            node.height = Val::Percent((pct * 100.0).max(6.0));
            *bg = BackgroundColor(if pct > 0.45 {
                COL_ACCENT_CYAN
            } else if pct > 0.15 {
                COL_ACCENT_AMBER
            } else {
                COL_ACCENT_RED
            });
        } else {
            node.height = Val::Percent(0.0);
        }
    }
    for (pt, mut t) in &mut pcts_q {
        if let Some(cell) = inv.battery_pouch.get(pt.0) {
            t.0 = format!("{}%", cell.charge_pct());
        } else {
            t.0 = "--".to_string();
        }
    }
}

// =============================================================================
// SETTINGS WIDGETS + OPTIONAL [F3] ACOUSTIC DIAGNOSTICS
// =============================================================================

fn truncate_device_name(name: Option<&str>) -> String {
    match name {
        None => "System Default".to_string(),
        Some(s) if s.len() > 32 => format!("{}...", &s[..29]),
        Some(s) => s.to_string(),
    }
}

#[allow(clippy::type_complexity)]
fn update_settings_widgets_system(
    user_settings: Res<UserSettings>,
    voice_settings: Res<VoiceSettings>,
    telemetry: Res<AcousticTelemetry>,
    updater: Res<UpdaterState>,
    mut slider_bars_q: Query<(&SliderFillBar, &mut Node)>,
    mut bg_set: ParamSet<(
        Query<(&SettingsMicLed, &mut BackgroundColor)>,
        Query<(&ToggleButtonPill, &mut BackgroundColor, &mut BorderColor)>,
    )>,
    mut text_set: ParamSet<(
        Query<(&SliderValueText, &mut Text)>,
        Query<&mut Text, With<OutputDeviceLabel>>,
        Query<&mut Text, With<InputDeviceLabel>>,
        Query<(&ToggleButtonLabel, &mut Text)>,
        Query<&mut Text, With<SettingsMicBadgeText>>,
        Query<&mut Text, With<SettingsUpdaterStatusText>>,
    )>,
) {
    for (bar, mut node) in &mut slider_bars_q {
        let frac = slider_current_frac(&user_settings, bar.0);
        node.width = Val::Percent((frac * 100.0).clamp(0.0, 100.0));
    }

    let mic = telemetry.mic;
    let norm_mic = ((mic.rms_db + 68.0) / 58.0).clamp(0.0, 1.0);
    let lit = if !mic.available || voice_settings.muted {
        0
    } else {
        (norm_mic * MIC_LED_BARS as f32).round() as usize
    };
    for (led, mut bg) in &mut bg_set.p0() {
        *bg = if led.0 < lit {
            if led.0 >= 7 {
                BackgroundColor(COL_ACCENT_RED)
            } else if led.0 >= 4 {
                BackgroundColor(COL_ACCENT_AMBER)
            } else {
                BackgroundColor(COL_ACCENT_GREEN)
            }
        } else {
            BackgroundColor(Color::srgba(0.14, 0.19, 0.24, 0.85))
        };
    }

    for (pill, mut bg, mut border) in &mut bg_set.p1() {
        let (base_col, hover_col, border_col) = match pill.kind {
            ToggleKind::Mute => {
                if voice_settings.muted {
                    (
                        Color::srgba(0.42, 0.11, 0.13, 0.98),
                        Color::srgba(0.56, 0.17, 0.19, 0.98),
                        COL_ACCENT_RED,
                    )
                } else {
                    (
                        Color::srgba(0.08, 0.28, 0.18, 0.96),
                        Color::srgba(0.12, 0.39, 0.25, 0.98),
                        COL_ACCENT_GREEN,
                    )
                }
            }
            ToggleKind::SelfEcho => {
                if voice_settings.self_echo_test {
                    (
                        Color::srgba(0.09, 0.30, 0.34, 0.98),
                        Color::srgba(0.14, 0.43, 0.47, 0.98),
                        COL_ACCENT_CYAN,
                    )
                } else {
                    (
                        Color::srgba(0.11, 0.16, 0.22, 0.96),
                        Color::srgba(0.18, 0.28, 0.36, 0.98),
                        COL_BORDER_DIM,
                    )
                }
            }
            ToggleKind::Fullscreen => {
                if user_settings.fullscreen {
                    (
                        Color::srgba(0.09, 0.30, 0.34, 0.98),
                        Color::srgba(0.14, 0.43, 0.47, 0.98),
                        COL_ACCENT_CYAN,
                    )
                } else {
                    (
                        Color::srgba(0.11, 0.16, 0.22, 0.96),
                        Color::srgba(0.18, 0.28, 0.36, 0.98),
                        COL_BORDER_DIM,
                    )
                }
            }
            ToggleKind::IncludePrereleases => {
                if user_settings.include_prereleases {
                    (
                        Color::srgba(0.09, 0.30, 0.34, 0.98),
                        Color::srgba(0.14, 0.43, 0.47, 0.98),
                        COL_ACCENT_CYAN,
                    )
                } else {
                    (
                        Color::srgba(0.11, 0.16, 0.22, 0.96),
                        Color::srgba(0.18, 0.28, 0.36, 0.98),
                        COL_BORDER_DIM,
                    )
                }
            }
        };
        *bg = BackgroundColor(if pill.hovered { hover_col } else { base_col });
        *border = BorderColor::all(border_col);
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
    for (lbl, mut t) in &mut text_set.p3() {
        t.0 = match lbl.0 {
            ToggleKind::Mute => {
                if voice_settings.muted {
                    "Mic: MUTED [M]".to_string()
                } else {
                    "Mic: LIVE [M]".to_string()
                }
            }
            ToggleKind::SelfEcho => {
                if voice_settings.self_echo_test {
                    "Self-Echo: ON [V]".to_string()
                } else {
                    "Self-Echo: OFF [V]".to_string()
                }
            }
            ToggleKind::Fullscreen => {
                if user_settings.fullscreen {
                    "Fullscreen [F11]".to_string()
                } else {
                    "Windowed [F11]".to_string()
                }
            }
            ToggleKind::IncludePrereleases => {
                if user_settings.include_prereleases {
                    "Pre-releases: ON".to_string()
                } else {
                    "Pre-releases: OFF".to_string()
                }
            }
        };
    }
    if let Ok(mut t) = text_set.p4().single_mut() {
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
        let echo = if voice_settings.self_echo_test {
            " • ECHO ON"
        } else {
            ""
        };
        t.0 = format!("{state} ({:.0} dB){echo}", mic.rms_db);
    }
    if let Ok(mut t) = text_set.p5().single_mut() {
        t.0 = updater.settings_status_text.clone();
    }
}

#[allow(clippy::type_complexity)]
fn update_audio_dropdown_system(
    user_settings: Res<UserSettings>,
    catalog: Res<AudioDeviceCatalog>,
    mut dropdown: ResMut<AudioDropdownState>,
    mut nodes: ParamSet<(
        Query<(&AudioDropdownList, &mut Node)>,
        Query<(&AudioDropdownRow, &mut Node, &mut BackgroundColor, &mut BorderColor)>,
        Query<(&AudioDropdownPagerRow, &mut Node)>,
    )>,
    mut texts: ParamSet<(
        Query<(&AudioDropdownChevron, &mut Text)>,
        Query<(&AudioDropdownRowText, &mut Text, &mut TextColor)>,
        Query<(&AudioDropdownPageLabel, &mut Text)>,
    )>,
) {
    let device_for = |kind: AudioDropdownKind, abs_idx: usize| -> Option<Option<&str>> {
        let list = match kind {
            AudioDropdownKind::Output => &catalog.output_devices,
            AudioDropdownKind::Input => &catalog.input_devices,
        };
        if abs_idx == 0 {
            Some(None)
        } else {
            list.get(abs_idx - 1).map(|s| Some(s.as_str()))
        }
    };
    let total_for = |kind: AudioDropdownKind| -> usize {
        match kind {
            AudioDropdownKind::Output => catalog.output_devices.len() + 1,
            AudioDropdownKind::Input => catalog.input_devices.len() + 1,
        }
    };
    let is_selected = |kind: AudioDropdownKind, entry: Option<&str>| -> bool {
        match kind {
            AudioDropdownKind::Output => user_settings.output_device.as_deref() == entry,
            AudioDropdownKind::Input => user_settings.input_device.as_deref() == entry,
        }
    };

    if let Some(open_kind) = dropdown.open {
        let total_pages = total_for(open_kind).div_ceil(AUDIO_DROPDOWN_ROWS).max(1);
        if dropdown.page >= total_pages {
            dropdown.page = total_pages - 1;
        }
    }

    for (ch, mut t) in &mut texts.p0() {
        t.0 = if dropdown.open == Some(ch.0) { "▲" } else { "▼" }.to_string();
    }

    for (list, mut node) in &mut nodes.p0() {
        node.display = if dropdown.open == Some(list.0) {
            Display::Flex
        } else {
            Display::None
        };
    }

    let Some(open_kind) = dropdown.open else {
        return;
    };

    let start = dropdown.page * AUDIO_DROPDOWN_ROWS;
    let total = total_for(open_kind);
    let total_pages = total.div_ceil(AUDIO_DROPDOWN_ROWS).max(1);

    for (row, mut node, mut bg, mut border) in &mut nodes.p1() {
        if row.kind != open_kind {
            node.display = Display::None;
            continue;
        }
        let abs_idx = start + row.idx;
        if let Some(entry) = device_for(open_kind, abs_idx) {
            node.display = Display::Flex;
            let selected = is_selected(open_kind, entry);
            *bg = BackgroundColor(if selected && row.hovered {
                Color::srgba(0.13, 0.34, 0.38, 0.98)
            } else if selected {
                Color::srgba(0.09, 0.24, 0.28, 0.98)
            } else if row.hovered {
                Color::srgba(0.13, 0.21, 0.29, 0.98)
            } else {
                COL_CARD_BG
            });
            *border = BorderColor::all(if selected || row.hovered {
                COL_ACCENT_CYAN
            } else {
                COL_BORDER_DIM
            });
        } else {
            node.display = Display::None;
        }
    }

    for (rt, mut t, mut col) in &mut texts.p1() {
        if rt.kind != open_kind {
            continue;
        }
        let abs_idx = start + rt.idx;
        if let Some(entry) = device_for(open_kind, abs_idx) {
            t.0 = match entry {
                None => "System Default".to_string(),
                Some(name) => truncate_device_name(Some(name)),
            };
            *col = TextColor(if is_selected(open_kind, entry) {
                COL_ACCENT_CYAN
            } else {
                COL_TEXT_BRIGHT
            });
        }
    }

    for (pager, mut node) in &mut nodes.p2() {
        node.display = if pager.0 == open_kind && total_pages > 1 {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (lbl, mut t) in &mut texts.p2() {
        if lbl.0 == open_kind {
            t.0 = format!("Page {} / {}", dropdown.page + 1, total_pages);
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_updater_popup_system(
    updater: Res<UpdaterState>,
    mut texts: ParamSet<(
        Query<&mut Text, With<UpdatePopupVersionText>>,
        Query<&mut Text, With<UpdatePopupNotesText>>,
        Query<(&mut Text, &mut TextColor), With<UpdatePopupStatusText>>,
        Query<&mut Text, With<UpdatePopupInstallLabel>>,
        Query<&mut Text, With<UpdatePopupDismissLabel>>,
    )>,
    mut progress_nodes: ParamSet<(
        Query<&mut Node, With<UpdatePopupProgressWrap>>,
        Query<(&mut Node, &mut BackgroundColor), With<UpdatePopupProgressFill>>,
    )>,
) {
    if !updater.popup_open {
        return;
    }

    if let Some(info) = updater.active_update_info()
        && let Ok(mut t) = texts.p0().single_mut()
    {
        let kind = if info.prerelease { "  •  Pre-release" } else { "" };
        t.0 = format!("{APP_VERSION}  →  {}{kind}", info.version_tag);
    }
    if let Ok(mut t) = texts.p1().single_mut() {
        t.0 = updater.notes_preview();
    }

    let (status_line, is_err) = updater.popup_status_line();
    let show_progress = !status_line.is_empty();
    if let Ok(mut wrap) = progress_nodes.p0().single_mut() {
        wrap.display = if show_progress {
            Display::Flex
        } else {
            Display::None
        };
    }
    if let Ok((mut t, mut col)) = texts.p2().single_mut() {
        t.0 = status_line;
        *col = TextColor(if is_err {
            COL_ACCENT_RED
        } else {
            COL_ACCENT_CYAN
        });
    }

    if let Ok((mut node, mut bg)) = progress_nodes.p1().single_mut() {
        let frac = updater.progress_fraction();
        node.width = Val::Percent((frac * 100.0).clamp(0.0, 100.0));
        *bg = BackgroundColor(if is_err {
            COL_ACCENT_RED
        } else if matches!(updater.status, UpdaterUiStatus::Restarting(_)) {
            COL_ACCENT_GREEN
        } else {
            COL_ACCENT_CYAN
        });
    }

    if let Ok(mut t) = texts.p3().single_mut() {
        t.0 = match updater.status {
            UpdaterUiStatus::Downloading { .. }
            | UpdaterUiStatus::Verifying(_)
            | UpdaterUiStatus::Extracting(_)
            | UpdaterUiStatus::Installing(_)
            | UpdaterUiStatus::Restarting(_) => "Updating…".to_string(),
            UpdaterUiStatus::Failed { .. } => "Retry".to_string(),
            _ => "Update & Restart".to_string(),
        };
    }
    if let Ok(mut t) = texts.p4().single_mut() {
        t.0 = if matches!(updater.status, UpdaterUiStatus::Failed { .. }) {
            "Close".to_string()
        } else {
            format!("Stay on {APP_VERSION}")
        };
    }
}

fn update_diagnostics_drawer_system(
    diag_open: Res<DiagnosticsDrawerOpen>,
    telemetry: Res<AcousticTelemetry>,
    mut diag_q: Query<&mut Text, With<DiagText>>,
) {
    if !diag_open.0 {
        return;
    }
    let rev = telemetry.reverb;
    let beacon = if let Some(b) = telemetry.beacon_params {
        let taps = b.early_taps.iter().filter(|t| t.active).count();
        format!(
            "Beacon {:.1}m | Wall {:.2}m | L/M/H {:.0}/{:.0}/{:.0} dB | Taps {}",
            b.distance_m,
            b.solid_thickness_m,
            linear_to_db(b.direct_gain.0[0]),
            linear_to_db(b.direct_gain.0[1]),
            linear_to_db(b.direct_gain.0[2]),
            taps
        )
    } else {
        "Beacon solving...".to_string()
    };
    if let Ok(mut t) = diag_q.single_mut() {
        t.0 = format!(
            "[F3] ACOUSTIC TELEMETRY\nRT60 L/M/H: {:.2}/{:.2}/{:.2}s  |  MFP {:.1}m\n{}\nControls: [E] Interact  [RMB] Use Held Item  [LMB] Pickaxe  [R] Battery  [Q] Drop  [B] Shop",
            rev.rt60_sec.0[0],
            rev.rt60_sec.0[1],
            rev.rt60_sec.0[2],
            rev.mean_free_path_m,
            beacon
        );
    }
}
