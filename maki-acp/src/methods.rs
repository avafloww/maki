use maki_agent::{ModeId, ModeRegistry};

use agent_client_protocol_schema::{
    AgentCapabilities, Implementation, InitializeResponse, LoadSessionResponse, McpCapabilities,
    NewSessionResponse, PromptCapabilities, ProtocolVersion, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOption, SessionMode, SessionModeId,
    SessionModeState,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const MODE_BUILD: &str = "build";
const MODE_PLAN: &str = "plan";

pub const MODEL_CONFIG_ID: &str = "model";

pub fn initialize_response() -> InitializeResponse {
    InitializeResponse::new(ProtocolVersion::V1)
        .agent_capabilities(
            AgentCapabilities::new()
                .load_session(true)
                .prompt_capabilities(PromptCapabilities::new().image(true).embedded_context(true))
                .mcp_capabilities(McpCapabilities::new().http(true)),
        )
        .auth_methods(vec![])
        .agent_info(Implementation::new("maki", VERSION))
}

pub fn mode_state(current: &str, modes: &ModeRegistry) -> SessionModeState {
    let mut list: Vec<SessionMode> = modes
        .list()
        .into_iter()
        .filter(|d| !matches!(d.id, ModeId::Custom(_)))
        .map(|d| {
            SessionMode::new(
                SessionModeId::from(d.id.key().to_string()),
                d.label.to_string(),
            )
        })
        .collect();
    for def in modes.list() {
        if let ModeId::Custom(name) = def.id {
            list.push(SessionMode::new(
                SessionModeId::from(name.to_string()),
                def.label.to_string(),
            ));
        }
    }
    SessionModeState::new(SessionModeId::from(current.to_string()), list)
}

pub fn new_session_response(session_id: &str, modes: &ModeRegistry) -> NewSessionResponse {
    NewSessionResponse::new(session_id.to_string()).modes(mode_state(MODE_BUILD, modes))
}

pub fn load_session_response(modes: &ModeRegistry) -> LoadSessionResponse {
    LoadSessionResponse::new().modes(mode_state(MODE_BUILD, modes))
}

pub fn model_config_option(current: &str, specs: &[String]) -> SessionConfigOption {
    let mut options: Vec<SessionConfigSelectOption> = specs
        .iter()
        .map(|spec| SessionConfigSelectOption::new(spec.clone(), spec.clone()))
        .collect();
    if !specs.iter().any(|spec| spec == current) {
        options.insert(
            0,
            SessionConfigSelectOption::new(current.to_string(), current.to_string()),
        );
    }
    SessionConfigOption::select(MODEL_CONFIG_ID, "Model", current.to_string(), options)
        .category(SessionConfigOptionCategory::Model)
}

pub fn mode_id_to_agent_mode(mode_id: &str, modes: &ModeRegistry) -> Option<maki_agent::AgentMode> {
    match mode_id {
        MODE_BUILD => Some(maki_agent::AgentMode::Build),
        MODE_PLAN => {
            let storage = maki_storage::StateDir::resolve().ok()?;
            let plan_path = maki_storage::plans::new_plan_path(&storage).ok()?;
            Some(maki_agent::AgentMode::Plan(plan_path))
        }
        name if modes.contains(name) => {
            Some(maki_agent::AgentMode::Custom(ModeId::Custom(name.into())))
        }
        _ => None,
    }
}
