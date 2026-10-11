//! The blocks a person reads in a terminal. Logos-free and unit-tested.

use serde_json::Value;

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

const WIDTH: usize = 64;

fn rule(c: char) -> String {
    std::iter::repeat_n(c, WIDTH).collect::<String>() + "\n"
}

fn money_lines(out: &mut String, summary: &Value) {
    for r in summary["recipients"].as_array().into_iter().flatten() {
        out.push_str(&format!("  Pay:     {}  {} BTC\n", s(r, "address"), s(r, "amountBtc")));
    }
    for c in summary["change"].as_array().into_iter().flatten() {
        out.push_str(&format!("  Change:  {}  {} BTC, back to this wallet\n", s(c, "address"), s(c, "amountBtc")));
    }
    let rate = summary["feeRate"].as_f64().map(|r| format!("{r} sat/vB")).unwrap_or_default();
    let vsize = summary["vsize"].as_u64().map(|v| format!(", {v} vB")).unwrap_or_default();
    out.push_str(&format!("  Fee:     {} BTC ({rate}{vsize})\n", s(summary, "feeBtc")));
    out.push_str(&format!("  Total:   {} BTC leaves the wallet\n", s(summary, "totalBtc")));
    if let Some(r) = summary["replaces"].as_str() {
        out.push_str(&format!("  Replaces {r}\n"));
    }
    for w in summary["warnings"].as_array().into_iter().flatten() {
        out.push_str(&format!("  ! {}\n", w["message"].as_str().or(w.as_str()).unwrap_or("a warning")));
    }
}

/// `status` is the backend's send_status reply for a send awaiting approval.
pub fn prompt(status: &Value, me: &str) -> String {
    let id = s(status, "requestId");
    let handle = status["approvalHandle"].as_str().unwrap_or("<handle>");
    let what = if s(status, "kind") == "fee_bump" { "BITCOIN FEE BUMP" } else { "BITCOIN SEND" };
    let mut out = rule('=');
    out.push_str(&format!("{what} AWAITING APPROVAL  {id}\n"));
    out.push_str(&rule('-'));
    out.push_str(&format!("Requested by: {}\n", s(status, "requester")));
    out.push_str("The wallet's own summary. The signer shows the signer manager's reading\n");
    out.push_str("of the same transaction; approve only what that says.\n");
    money_lines(&mut out, &status["summary"]);
    out.push_str(&rule('-'));
    out.push_str(&format!("show:     logosctl call evm_signer_cli show {handle}\n"));
    out.push_str(&format!("approve:  logosctl call evm_signer_cli approve {handle} <bundle id> @/path/to/pwfile\n"));
    out.push_str(&format!("reject:   logosctl call evm_signer_cli reject {handle}\n"));
    out.push_str(&format!("withdraw: logosctl call {me} cancel_send {id}\n"));
    out.push_str("An offer nobody acknowledges within 60 s expires; request the send again then.\n");
    out.push_str(&rule('='));
    out
}

/// Any send, in whatever state it is.
pub fn show(status: &Value, me: &str) -> String {
    if s(status, "state") == "awaiting_approval" {
        return prompt(status, me);
    }
    let id = s(status, "requestId");
    let mut out = rule('=');
    out.push_str(&format!("BITCOIN {} {id}: {}\n", if s(status, "kind") == "fee_bump" { "FEE BUMP" } else { "SEND" }, s(status, "state")));
    out.push_str(&rule('-'));
    if !status["summary"].is_null() {
        money_lines(&mut out, &status["summary"]);
    }
    if let Some(t) = status["txid"].as_str() {
        out.push_str(&format!("  Txid:    {t}\n"));
    }
    if let Some(b) = status["broadcast"].as_str() {
        out.push_str(&format!("  Broadcast: {b}{}\n", status["via"].as_str().map(|v| format!(" via {v}")).unwrap_or_default()));
    }
    if let Some(e) = status["error"].as_str() {
        out.push_str(&format!("  {e}\n"));
    }
    if s(status, "state") == "unknown" {
        out.push_str(&rule('-'));
        out.push_str(&format!("rebroadcast: logosctl call {me} rebroadcast {id}\n"));
        let txid = status["txid"].as_str().unwrap_or("<txid>");
        out.push_str(&format!("fee bump:    logosctl call {me} request_fee_bump '{{\"txid\":\"{txid}\",\"feeRate\":<sat/vB>}}'\n"));
    }
    out.push_str(&rule('='));
    out
}

/// How the person decides an open, headless, with the keys CLI.
pub fn open_next(handle: Option<&str>) -> String {
    let h = handle.unwrap_or("<sma_ handle from wallet_status>");
    format!(
        "The person picks, creates or restores the wallet and approves the open in the keys app. Headless: \
         logosctl call evm_keystore_cli access_requests; logosctl call evm_keystore_cli show_access {h}; \
         logosctl call evm_keystore_cli approve_access @decision.json with {{handle, bundle_id, group?, password, unlock?}}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn awaiting() -> Value {
        json!({"ok": true, "requestId": "s1", "kind": "send", "state": "awaiting_approval", "requester": "bitcoin_wallet_cli",
               "approvalHandle": "smh_00000002",
               "summary": {"recipients": [{"address": "bcrt1qpay", "amountSat": "60000", "amountBtc": "0.00060000"}],
                           "change": [{"address": "bcrt1qchange", "amountSat": "39859", "amountBtc": "0.00039859"}],
                           "feeBtc": "0.00000141", "feeRate": 1.0, "vsize": 141, "totalBtc": "0.00060141", "replaces": null,
                           "warnings": [{"kind": "change_too_small", "message": "The change is worth less than it costs to spend."}]}})
    }

    #[test]
    fn the_prompt_names_the_money_the_handle_and_the_signer_commands() {
        let t = prompt(&awaiting(), "bitcoin_wallet_cli");
        assert!(t.contains("BITCOIN SEND AWAITING APPROVAL  s1"));
        assert!(t.contains("Pay:     bcrt1qpay  0.00060000 BTC"));
        assert!(t.contains("Change:  bcrt1qchange  0.00039859 BTC, back to this wallet"));
        assert!(t.contains("Fee:     0.00000141 BTC (1 sat/vB, 141 vB)"));
        assert!(t.contains("Total:   0.00060141 BTC leaves the wallet"));
        assert!(t.contains("logosctl call evm_signer_cli approve smh_00000002 <bundle id> @/path/to/pwfile"));
        assert!(t.contains("logosctl call evm_signer_cli show smh_00000002"));
        assert!(t.contains("logosctl call bitcoin_wallet_cli cancel_send s1"));
        assert!(t.contains("! The change is worth less"));
        assert!(t.contains("60 s"));
        assert!(!t.to_lowercase().contains("password:"), "the CLI never asks for one");
    }

    #[test]
    fn an_unknown_send_offers_rebroadcast_and_a_fee_bump() {
        let st = json!({"requestId": "s2", "kind": "send", "state": "unknown", "txid": "ab", "broadcast": "unknown",
                        "error": "the transaction may have left", "summary": null});
        let t = show(&st, "bitcoin_wallet_cli");
        assert!(t.contains("BITCOIN SEND s2: unknown"));
        assert!(t.contains("logosctl call bitcoin_wallet_cli rebroadcast s2"));
        assert!(t.contains(r#"request_fee_bump '{"txid":"ab","feeRate":<sat/vB>}'"#));
        assert!(show(&awaiting(), "bitcoin_wallet_cli").contains("AWAITING APPROVAL"));
    }

    #[test]
    fn the_open_hint_names_the_keys_cli() {
        let t = open_next(Some("sma_00000001"));
        assert!(t.contains("logosctl call evm_keystore_cli show_access sma_00000001"));
        assert!(t.contains("approve_access @decision.json"));
        assert!(open_next(None).contains("wallet_status"));
    }
}
