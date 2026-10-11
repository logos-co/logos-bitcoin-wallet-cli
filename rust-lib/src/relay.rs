//! Logos-free helpers: the configure that would admit this module, and the refusal rewritten
//! to name it.

use serde_json::{json, Value};

pub const BACKEND: &str = "bitcoin_wallet_backend";

fn custodians(identity: &Value) -> Vec<String> {
    identity["custodians"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

pub fn enrolled(identity: &Value, me: &str) -> bool {
    custodians(identity).iter().any(|c| c == me)
}

/// The `configure` document that adds `me` and keeps every custodian already in force:
/// configure is total, so a document naming only `me` would strip the app.
pub fn configure_doc(identity: &Value, me: &str) -> String {
    let mut list = custodians(identity);
    if !list.iter().any(|c| c == me) {
        list.push(me.to_string());
    }
    json!({"custodians": list}).to_string()
}

/// The exact fix. Only a custodian may call configure and logosctl never is one, so a
/// headless operator writes the same document to the backend's roles.json and reloads it.
pub fn fix(identity: &Value, me: &str) -> String {
    let doc = configure_doc(identity, me);
    let holders = custodians(identity);
    let who = if holders.is_empty() { "a custodian".to_string() } else { format!("a custodian ({})", holders.join(", ")) };
    let file = identity["rolesFile"].as_str().unwrap_or("roles.json in bitcoin_wallet_backend's persistence directory");
    // A reload keeps dependents such as this module loaded; an unload would take them down too.
    format!("configure '{doc}' — {who} calls it; headless, write that document to {file}, then: logosctl module reload {BACKEND}")
}

pub fn not_custodian(me: &str, role_word: &str, fix: &str) -> String {
    format!("not authorized: {me} is not {role_word} of {BACKEND}. The fix: {fix}")
}

/// The backend's one opaque refusal, rewritten to name the fix; anything else passes through.
pub fn translate_refusal(reply: &str, me: &str, role_word: &str, fix: impl FnOnce() -> String) -> String {
    let Ok(mut v) = serde_json::from_str::<Value>(reply) else { return reply.to_string() };
    if v["ok"] == false && v["error"] == "not authorized" {
        v["error"] = Value::String(not_custodian(me, role_word, &fix()));
        return v.to_string();
    }
    reply.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "bitcoin_wallet_cli";

    #[test]
    fn the_fix_keeps_the_app_and_adds_this_module_once() {
        let id = json!({"custodians": ["bitcoin_wallet_ui"], "rolesFile": "/x/roles.json"});
        assert_eq!(configure_doc(&id, ME), r#"{"custodians":["bitcoin_wallet_ui","bitcoin_wallet_cli"]}"#);
        let f = fix(&id, ME);
        assert!(f.starts_with(r#"configure '{"custodians":["bitcoin_wallet_ui","bitcoin_wallet_cli"]}'"#), "{f}");
        assert!(f.contains("a custodian (bitcoin_wallet_ui) calls it"));
        assert!(f.contains("write that document to /x/roles.json"));
        assert!(f.ends_with("then: logosctl module reload bitcoin_wallet_backend"), "{f}");
        let held = json!({"custodians": ["bitcoin_wallet_ui", ME]});
        assert_eq!(configure_doc(&held, ME).matches(ME).count(), 1);
        assert!(enrolled(&held, ME) && !enrolled(&id, ME));
    }

    #[test]
    fn an_empty_or_unreadable_identity_still_gives_a_fix() {
        assert_eq!(configure_doc(&json!({"custodians": []}), ME), r#"{"custodians":["bitcoin_wallet_cli"]}"#);
        let f = fix(&Value::Null, ME);
        assert!(f.contains("a custodian calls it") && f.contains("persistence directory"), "{f}");
    }

    #[test]
    fn only_the_opaque_refusal_is_rewritten() {
        let out = translate_refusal(r#"{"ok":false,"error":"not authorized"}"#, ME, "a custodian", || "F".into());
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "not authorized: bitcoin_wallet_cli is not a custodian of bitcoin_wallet_backend. The fix: F");
        let other = r#"{"ok":false,"error":"no wallet is open"}"#;
        assert_eq!(translate_refusal(other, ME, "a custodian", || panic!("not asked")), other);
        assert_eq!(translate_refusal("not json", ME, "a custodian", || panic!("not asked")), "not json");
        let fine = r#"{"ok":true}"#;
        assert_eq!(translate_refusal(fine, ME, "a custodian", || panic!("not asked")), fine);
    }
}
