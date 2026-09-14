//! r1.s2.w1 — the cancellation contract's core surface.
//!
//! Pins the wire forms of the two new `AgentOutcome` variants and the
//! `ToolContext.cancel` observation guarantee a tool body relies on: a
//! token handed to an invocation is a child of the run token, so cancelling
//! the parent is observed inside the tool. Nothing here cancels a run —
//! w2–w4 add the behavior; this item locks the types and the plumbing the
//! behavior hangs off.

use pulsehive_core::agent::AgentOutcome;
use pulsehive_core::event::EventEmitter;
use pulsehive_core::ids::CollectiveId;
use pulsehive_core::tool::ToolContext;
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// Builds a `ToolContext` carrying `cancel`; the substrate/emitter are never
/// touched by these tests. Mirrors `tool::tests::test_context` — a temp
/// `Config::default()` PulseDB avoids the ONNX path. The `substrate` field is
/// cfg-gated because `ToolContext` only carries it under the feature, which a
/// `--workspace` build unifies on through the runtime/bindings crates.
fn context_with_cancel(cancel: CancellationToken) -> ToolContext {
    #[cfg(feature = "substrate")]
    let substrate = {
        let dir = tempfile::tempdir().unwrap();
        let db =
            pulsedb::PulseDB::open(dir.path().join("test.db"), pulsedb::Config::default()).unwrap();
        // Leak the tempdir so its files outlive the context.
        Box::leak(Box::new(dir));
        std::sync::Arc::new(pulsedb::PulseDBSubstrate::from_db(db))
    };
    ToolContext {
        agent_id: "agent-test".into(),
        collective_id: CollectiveId::new(),
        #[cfg(feature = "substrate")]
        substrate,
        event_emitter: EventEmitter::default(),
        cancel,
    }
}

#[test]
fn cancelled_serializes_to_tagged_wire_form() {
    let outcome = AgentOutcome::Cancelled {
        partial_response: "half an answer".into(),
    };

    let value = serde_json::to_value(&outcome).unwrap();
    assert_eq!(
        value,
        json!({"status": "cancelled", "partial_response": "half an answer"})
    );

    let back: AgentOutcome = serde_json::from_value(value).unwrap();
    assert!(
        matches!(back, AgentOutcome::Cancelled { partial_response } if partial_response == "half an answer")
    );
}

#[test]
fn partial_complete_serializes_to_tagged_wire_form() {
    let outcome = AgentOutcome::PartialComplete {
        responses: vec!["first child".into(), "third child".into()],
        errors: vec!["second child: max iterations reached".into()],
    };

    let value = serde_json::to_value(&outcome).unwrap();
    assert_eq!(
        value,
        json!({
            "status": "partial_complete",
            "responses": ["first child", "third child"],
            "errors": ["second child: max iterations reached"],
        })
    );

    let back: AgentOutcome = serde_json::from_value(value).unwrap();
    assert!(
        matches!(back, AgentOutcome::PartialComplete { responses, errors }
            if responses == ["first child", "third child"] && errors == ["second child: max iterations reached"])
    );
}

#[test]
fn tool_context_child_token_observes_parent_cancellation() {
    let parent = CancellationToken::new();
    let ctx = context_with_cancel(parent.child_token());

    assert!(!ctx.cancel.is_cancelled());
    parent.cancel();
    assert!(ctx.cancel.is_cancelled());
}

#[tokio::test]
async fn tool_context_cancelled_future_resolves_when_parent_cancels() {
    let parent = CancellationToken::new();
    let ctx = context_with_cancel(parent.child_token());

    // Cancel from a separate task so the await genuinely pends first.
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        parent.cancel();
    });

    ctx.cancel.cancelled().await;
    assert!(ctx.cancel.is_cancelled());
}
