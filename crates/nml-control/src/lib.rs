//! Control channel of the `nml-viewer`: protocol types, line codec, key names and Base64. Without
//! winit and GPU; shared by the viewer and the MCP server `nml-viewer-mcp`
//! (description in `docs/viewer-control.md`).

mod addr;
pub mod base64;
mod codec;
mod keys;
mod protocol;
mod sim;
mod state;
mod ui;

pub use addr::parse_control_addr;
pub use codec::{
    LineRead, MAX_REQUEST_LINE, MAX_RESPONSE_LINE, parse_request, read_line, write_line,
};
pub use keys::{KEY_NAMES, UnknownKey, check_key};
pub use protocol::MAX_KI_QUERY_WORDS;
pub use protocol::{
    CAMPAIGN_DIFFICULTY_RANGE, DEFAULT_SCRIPT_LOG_STEPS, KI_KIND_RANGE, KI_STACK_TOP, KiAction,
    MAX_KI_ARGS, MAX_KI_STEP_PASSES, MAX_SCRIPT_LOG_STEPS, MAX_SLOT_HANDICAP, MAX_SLOTS,
    ScriptAction, ScriptBlockRef, SlotSetup, SlotSetupKind, TIME_LIMIT_RANGE, TIME_PER_CHAR_RANGE,
    check_slots,
};
pub use protocol::{
    CaptureCommand, CaptureSummaryData, Command, GRID_OCCUPANCY, HelloInfo, KeyAction,
    MAX_CANCEL_INDEX, MAX_GAME_SPEED, MAX_GRID_CELLS, MAX_LIVING_SPACE, MAX_QUALITY,
    MAX_QUERY_LIMIT, MAX_REPLAY_STEPS, MAX_RESOURCE_SETTING, MAX_ROTATION_STEPS, MAX_STEP_FRAMES,
    MAX_UPGRADE_SLOT, MAX_VOLUME, MouseButtonName, MusicCommand, NetDebugAction, Outcome,
    PROTOCOL_VERSION, PlaySoundResult, Request, Response, ScreenshotData, VIEWER_GONE, WeatherName,
    WindowModeName,
};
pub use sim::{KiCommandCount, KiEngineInfo, KiRejectCount, KiStateResult, KiThreadInfo};
pub use sim::{KiDirectInfo, KiQueryResult, SimAttackReport};
pub use sim::{
    ScriptActionCount, ScriptBlockInfo, ScriptCommandsInfo, ScriptConditionInfo,
    ScriptContainerInfo, ScriptEventEntry, ScriptEventsResult, ScriptLineInfo, ScriptLogEntry,
    ScriptQuestInfo, ScriptRejection, ScriptSpeechInfo, ScriptStateResult, ScriptTipInfo,
    ScriptUiInfo,
};
pub use sim::{
    SimActionResult, SimAnimalInfo, SimAreaView, SimAreaWatchInfo, SimAssassinInfo, SimAttackInfo,
    SimBanishInfo, SimBombermanInfo, SimBuildResult, SimBuildingInfo, SimCombatInfo,
    SimConstructInfo, SimCounterUpgrade, SimDivingInfo, SimDoorTile, SimDyingInfo, SimEdibleInfo,
    SimEvokeAnimalInfo, SimEvokeGhostInfo, SimFlagInfo, SimFlagsInfo, SimFogResult,
    SimFormationInfo, SimGateInfo, SimGathererInfo, SimGhostInfo, SimGridResult, SimGroupInfo,
    SimGroupTarget, SimHashResult, SimHeadhunterInfo, SimHealingInfo, SimHireEntry,
    SimHireMenuResult, SimInfluenceInfo, SimKillerInfo, SimLookInfo, SimMaskingInfo, SimMenuEntry,
    SimMenuResult, SimMessageFilterInfo, SimModuleInfo, SimMultiPartInfo, SimNotice, SimObjectInfo,
    SimPathResult, SimPlayerInfo, SimProductionInfo, SimProjectileInfo, SimQuarterInfo,
    SimQueryResult, SimQueueEntry, SimRangedInfo, SimResourceInfo, SimRidingInfo, SimRobbingInfo,
    SimSaveResult, SimSection, SimShootInfo, SimSpecialMoveInfo, SimSpellInfo, SimTargetMarker,
    SimTempUpgrade, SimTentInfo, SimTradeInfo, SimTrainInfo, SimTransportTentInfo, SimTreeInfo,
    SimWalkInfo, hex_checksum,
};
pub use state::{
    AnimationState, AspectName, AudioState, BeamState, BrowserState, CameraState, CaptureState,
    ChargeState, ClockMode, ClockState, CursorState, EffectBrowserState, EffectState,
    EnvironmentState, FrameStats, FrameTimes, FreeCameraState, HoveredObject, MarkCount,
    MusicState, NetFoundData, NetHaltName, NetLinkData, NetLobbyData, NetLobbyRowData,
    NetMigrationData, NetMigrationPhaseName, NetPartHash, NetPartHashesData, NetPeerData,
    NetPhaseName, NetRoleName, NetRttData, NetSeatData, NetStatusData, NetSuccessorData,
    NetWaitName, OrbitState, PendingSwitch, RallyFlagState, RandomMapState, RecentSoundState,
    RecentVoiceState, ShadowState, StateSnapshot, Toggles, ViewName, VoiceState, WindowState,
    WorldDisplayState,
};
pub use ui::{
    GAME_UI_DIALOG, UI_CLICK_MIDDLE, UiAccountingRow, UiDialogData, UiEffectData, UiEffectKind,
    UiElementData, UiGameData, UiHudData, UiHudMessage, UiHudWindow, UiInputData, UiMenuLevelData,
    UiMinimapData, UiPointerModelData, UiPortraitData, UiPortraitSlot, UiPresentationData,
    UiReplayData, UiReplayMode, UiSlotData, UiStateData, UiStatusBarData, UiTextSlot,
    UiToolbarButton, UiToolbarData, UiTooltipData, UiVideoData,
};
