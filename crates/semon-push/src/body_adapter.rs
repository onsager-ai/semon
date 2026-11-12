//! Origin-local typed view adapter. It never interprets remote opaque IDs as UIDs
//! or filesystem paths. The trusted admitted-source integration supplies that map.
use crate::origin_read::{
    Field, PageRenderer, PinnedPolicy, ReadClock, Refused, RenderBudget, RenderPlan, RenderedPage,
    ResolvedSource, Target, TypedField, TypedPage, UntrustedText,
};
use semon_sessions::body_query::{self, Access, Request};
use serde_json::Value;
use std::time::Duration;

type Result<T> = std::result::Result<T, Refused>;

/// Origin-owned exact target lookup, downstream of HandleResolver and governance.
/// Implementations must select admitted local indexes and an exact body/step/turn
/// UID; they must not approximate an anchor, use remote paths, or replay sessions.
pub trait LocalView {
    fn request(&self, source: &ResolvedSource, target: &Target) -> Result<Request>;
}
pub struct BodyQueryRenderer<'a> {
    pub views: &'a dyn LocalView,
    pub clock: &'a dyn ReadClock,
}
impl PageRenderer for BodyQueryRenderer<'_> {
    fn prepare(
        &self,
        source: &ResolvedSource,
        target: &Target,
        policy: PinnedPolicy,
        budget: RenderBudget,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<RenderPlan> {
        // body_query has no metadata-only canonical length query. This bounded
        // render is discarded immediately, never cached or logged. No mapping or
        // export witness is fabricated: OriginRead refuses before authorization
        // or egress while either is missing.
        let rendered = self.render(source, target, policy, budget, cancelled)?;
        Ok(RenderPlan {
            payload_bytes: rendered.page.canonical_bytes()?.len() as u64,
            export_view_generation: None,
            exact_mapping: None,
            continuation: None,
        })
    }
    fn render(
        &self,
        source: &ResolvedSource,
        target: &Target,
        _: PinnedPolicy,
        budget: RenderBudget,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<RenderedPage> {
        check(self.clock, budget, cancelled)?;
        let request = self.views.request(source, target)?;
        // Never let an integration select search or expand beyond focused view.
        if request.tool != "view" {
            return Err(Refused::Unavailable);
        }
        let remaining = budget
            .deadline_millis
            .saturating_sub(self.clock.now_millis());
        let timeout = Duration::from_millis(remaining.min(body_query::TIMEOUT_MS));
        if timeout.is_zero() {
            return Err(Refused::Deadline);
        }
        // SQLite's progress handler honors the narrowed deadline. The API does
        // not expose an interrupt/cancellation callback, so check on both sides.
        let value = body_query::execute_with_timeout(request, Access::Origin, timeout)
            .map_err(|_| Refused::Unavailable)?;
        check(self.clock, budget, cancelled)?;
        let page = typed_page(value, budget.bytes.min(body_query::VIEW_BYTES))?;
        Ok(RenderedPage {
            page,
            export_view_generation: None,
            exact_mapping: None,
            continuation: None,
        })
    }
}
fn check(clock: &dyn ReadClock, budget: RenderBudget, cancelled: &dyn Fn() -> bool) -> Result<()> {
    if cancelled() {
        return Err(Refused::Unavailable);
    }
    if clock.now_millis() >= budget.deadline_millis {
        return Err(Refused::Deadline);
    }
    if budget.bytes == 0 {
        return Err(Refused::OverBudget);
    }
    Ok(())
}
fn typed_page(mut value: Value, limit: usize) -> Result<TypedPage> {
    if value.get("truncated").and_then(Value::as_bool) != Some(false) {
        return Err(Refused::Unavailable);
    }
    let bodies = value
        .get_mut("bodies")
        .and_then(Value::as_array_mut)
        .ok_or(Refused::Unavailable)?;
    let mut fields = Vec::with_capacity(bodies.len());
    for body in bodies {
        let field = match body.get("part").and_then(Value::as_str) {
            Some("user") => Field::User,
            Some("assistant") => Field::Assistant,
            Some("command") => Field::Command,
            Some("tool_output") => Field::ToolOutput,
            Some("error") => Field::Error,
            _ => return Err(Refused::Unavailable),
        };
        let wrapper = body
            .get_mut("text")
            .and_then(Value::as_object_mut)
            .ok_or(Refused::Unavailable)?;
        if wrapper.len() != 1 {
            return Err(Refused::Unavailable);
        }
        let Some(Value::String(text)) = wrapper.remove("untrusted_text") else {
            return Err(Refused::Unavailable);
        };
        fields.push(TypedField {
            field,
            text: UntrustedText::new(text),
        });
    }
    let page = TypedPage { fields };
    if page.canonical_bytes()?.len() > limit {
        return Err(Refused::OverBudget);
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn preserves_typed_untrusted_text_and_refuses_inexact_views() {
        let value = json!({"truncated":false,"bodies":[{"part":"assistant","text":{"untrusted_text":"<script>data</script>"}}]});
        let page = typed_page(value.clone(), body_query::VIEW_BYTES).unwrap();
        assert_eq!(page.fields[0].text.as_str(), "<script>data</script>");
        assert_eq!(
            format!("{:?}", page.fields[0].text),
            "UntrustedText([redacted])"
        );
        assert!(matches!(
            typed_page(value.clone(), 1),
            Err(Refused::OverBudget)
        ));
        let mut cut = value.clone();
        cut["truncated"] = true.into();
        assert!(matches!(
            typed_page(cut, body_query::VIEW_BYTES),
            Err(Refused::Unavailable)
        ));
        let mut unknown = value.clone();
        unknown["bodies"][0]["part"] = "unknown".into();
        assert!(typed_page(unknown, body_query::VIEW_BYTES).is_err());
        let mut plain = value;
        plain["bodies"][0]["text"] = "plain".into();
        assert!(typed_page(plain, body_query::VIEW_BYTES).is_err());
    }
    #[test]
    fn real_view_entry_refuses_without_admitted_indexes() {
        let result = body_query::execute_with_timeout(
            Request {
                indexes: Vec::new(),
                tool: "view".into(),
                arguments: json!({"uid":"unadmitted"}),
            },
            Access::Origin,
            Duration::from_millis(1),
        );
        assert!(result.is_err());
    }
    #[test]
    fn cancellation_and_deadline_precede_local_lookup() {
        struct At;
        impl ReadClock for At {
            fn now_millis(&self) -> u64 {
                10
            }
        }
        let budget = RenderBudget {
            bytes: 10,
            deadline_millis: 10,
        };
        assert!(matches!(
            check(&At, budget, &|| false),
            Err(Refused::Deadline)
        ));
        assert!(matches!(
            check(&At, budget, &|| true),
            Err(Refused::Unavailable)
        ));
    }
}
