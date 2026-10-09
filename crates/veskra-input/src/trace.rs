//! JSONL event traces (one [`RawPointerEvent`] per line) for fixtures and replay harnesses.

use crate::{InputError, RawPointerEvent};

/// Maximum events accepted from one trace (bounded allocation).
pub const MAX_TRACE_EVENTS: usize = 2_000_000;

/// Parse a JSONL trace. Blank lines are skipped; every event is boundary-validated.
pub fn parse_jsonl(text: &str) -> Result<Vec<RawPointerEvent>, InputError> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line_no = i + 1;
        if line.trim().is_empty() {
            continue;
        }
        if out.len() >= MAX_TRACE_EVENTS {
            return Err(InputError::Trace { line: line_no, message: "too many events".into() });
        }
        let ev: RawPointerEvent = serde_json::from_str(line).map_err(|e| InputError::Trace {
            line: line_no,
            message: format!("parse error at column {}", e.column()),
        })?;
        ev.validate_at(out.len()).map_err(|e| InputError::Trace { line: line_no, message: e.to_string() })?;
        out.push(ev);
    }
    Ok(out)
}

/// Serialize events as JSONL (deterministic: fixed field order, shortest round-trip floats).
pub fn to_jsonl(events: &[RawPointerEvent]) -> Result<String, InputError> {
    let mut s = String::new();
    for (i, ev) in events.iter().enumerate() {
        ev.validate_at(i)?;
        let line =
            serde_json::to_string(ev).map_err(|_| InputError::Trace { line: i + 1, message: "serialize".into() })?;
        s.push_str(&line);
        s.push('\n');
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PointerAction;
    use veskra_core::ToolType;

    #[test]
    fn roundtrip_and_compact() {
        let mut a = RawPointerEvent::new(PointerAction::Down, 0, ToolType::Stylus, 1.5, 2.0, 10);
        a.pressure = Some(0.25);
        let mut b = RawPointerEvent::new(PointerAction::Move, 0, ToolType::Stylus, 2.5, 2.0, 20);
        b.historical = true;
        let s = to_jsonl(&[a, b]).unwrap();
        assert_eq!(
            s.lines().next().unwrap(),
            r#"{"action":"Down","pointer_id":0,"tool":"Stylus","x":1.5,"y":2.0,"t_ns":10,"pressure":0.25}"#
        );
        assert_eq!(parse_jsonl(&format!("{s}\n\n")).unwrap(), vec![a, b]);
    }

    #[test]
    fn rejects_bad_lines() {
        assert!(matches!(parse_jsonl("{"), Err(InputError::Trace { line: 1, .. })));
        let unknown = r#"{"action":"Down","pointer_id":0,"tool":"Stylus","x":1,"y":2,"t_ns":1,"bogus":1}"#;
        assert!(parse_jsonl(unknown).is_err());
        let both = r#"{"action":"Move","pointer_id":0,"tool":"Stylus","x":1,"y":2,"t_ns":1,"historical":true,"predicted":true}"#;
        assert!(matches!(parse_jsonl(both), Err(InputError::Trace { line: 1, .. })));
    }
}
