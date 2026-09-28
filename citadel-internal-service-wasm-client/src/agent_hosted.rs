//! Accounts whose ILM the AGENT runs, not this browser (docs/plans/multi-browser-cid.md, phase 2c).
//!
//! Two ILMs for one account would each acknowledge and record the same frames and
//! read-modify-write the same stored queues. This client is the one place a browser ILM is
//! created (`multiplex`, from `open_messenger_for` / `ensure_messenger_open`), so the rule lives
//! here: a CID marked agent-hosted is never multiplexed, and a CID whose browser ILM is running or
//! opening cannot be marked.
//!
//! The UI marks a CID only after a FRESH `GetSessions` shows the agent's offer, and before it sends
//! `EnableAgentIlm`; if the opt-in fails it unmarks and opens as before. Its messages then go out
//! as `SendReliable` through `send_direct_to_internal_service`, with a request id the UI awaits.
//! Inbound needs nothing here: the agent delivers unwrapped bodies, which the messenger already
//! forwards as non-ILM traffic.
use wasm_bindgen::prelude::*;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MarkRefused {
    BrowserIlmRunning,
    BrowserIlmOpening,
}

impl MarkRefused {
    fn reason(&self) -> &'static str {
        match self {
            Self::BrowserIlmRunning => {
                "this browser already runs this account's ILM; it cannot also be agent-hosted"
            }
            Self::BrowserIlmOpening => {
                "this browser is opening this account's ILM; it cannot also be agent-hosted"
            }
        }
    }
}

/// Whether a CID may become agent-hosted, given this browser's ILM state for it.
pub(crate) fn may_mark(
    browser_ilm_running: bool,
    browser_ilm_opening: bool,
) -> Result<(), MarkRefused> {
    if browser_ilm_running {
        return Err(MarkRefused::BrowserIlmRunning);
    }
    if browser_ilm_opening {
        return Err(MarkRefused::BrowserIlmOpening);
    }
    Ok(())
}

fn parse_cid(cid_str: &str) -> Result<u64, JsValue> {
    cid_str
        .parse()
        .map_err(|e| JsValue::from_str(&format!("Invalid CID format: {}", e)))
}

/// Mark `cid` agent-hosted. Refused while this browser runs or is opening an ILM for it.
///
/// Takes the state's WRITE lock: the open path checks and claims a CID under the read lock, so
/// holding the write lock here makes the check-and-mark exclusive with it.
#[wasm_bindgen]
pub async fn mark_agent_hosted(cid_str: String) -> Result<(), JsValue> {
    let cid: u64 = parse_cid(&cid_str)?;
    let workspace_state = crate::get_workspace_state();
    let guard = workspace_state.write().await;
    let state = guard
        .as_ref()
        .ok_or_else(|| JsValue::from_str("Workspace not initialized"))?;
    may_mark(
        state.connections.contains_key(&cid),
        state.pending_opens.contains(&cid),
    )
    .map_err(|refused| JsValue::from_str(refused.reason()))?;
    state.agent_hosted.insert(cid);
    Ok(())
}

/// Undo a mark, e.g. after the agent refused the opt-in, so the ordinary open can proceed.
#[wasm_bindgen]
pub async fn unmark_agent_hosted(cid_str: String) -> Result<(), JsValue> {
    let cid: u64 = parse_cid(&cid_str)?;
    let workspace_state = crate::get_workspace_state();
    let guard = workspace_state.read().await;
    let state = guard
        .as_ref()
        .ok_or_else(|| JsValue::from_str("Workspace not initialized"))?;
    state.agent_hosted.remove(&cid);
    Ok(())
}

#[wasm_bindgen]
pub async fn is_agent_hosted(cid_str: String) -> Result<bool, JsValue> {
    let cid: u64 = parse_cid(&cid_str)?;
    let workspace_state = crate::get_workspace_state();
    let guard = workspace_state.read().await;
    let state = guard
        .as_ref()
        .ok_or_else(|| JsValue::from_str("Workspace not initialized"))?;
    Ok(state.agent_hosted.contains(&cid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cid_with_no_browser_ilm_may_be_agent_hosted() {
        assert_eq!(may_mark(false, false), Ok(()));
    }

    #[test]
    fn a_running_or_opening_browser_ilm_refuses_the_mark() {
        assert_eq!(may_mark(true, false), Err(MarkRefused::BrowserIlmRunning));
        assert_eq!(may_mark(false, true), Err(MarkRefused::BrowserIlmOpening));
        assert_eq!(may_mark(true, true), Err(MarkRefused::BrowserIlmRunning));
    }
}
