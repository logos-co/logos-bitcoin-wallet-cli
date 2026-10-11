//! Logos glue for `bitcoin_wallet_cli`: relays to `bitcoin_wallet_backend` as itself, and a poll
//! thread turns its send list into `prompt`, `settled` and `queue_changed`. Never a password.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::sync::OnceLock;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use logos_rust_sdk::{AboutToUnload, LogosError, Shutdown};
use serde_json::{json, Value};

use crate::relay::{enrolled, fix, translate_refusal};
use crate::render;
use crate::watch::{Row, Watch};

const ME: &str = "bitcoin_wallet_cli";

pub trait BitcoinWalletCliModule: Send + Sync + 'static {
    /// `{ ok, enrolled, identity, custodians, rolesFile, awaiting, fix }`: whether the backend holds
    /// this module as a custodian, the sends awaiting approval, and the exact fix when it doesn't.
    fn status(&self) -> String;

    fn caller_identity(&self) -> String;
    fn list_networks(&self) -> String;
    fn list_wallets(&self) -> String;
    fn wallet_status(&self) -> String;
    fn balances(&self) -> String;
    fn history(&self, page: i64) -> String;
    fn receive_address(&self) -> String;
    fn fee_rates(&self) -> String;
    fn address_valid(&self, text: String) -> String;
    fn format_btc(&self, sat: String) -> String;
    fn parse_btc(&self, btc: String) -> String;
    fn sources(&self) -> String;
    fn source_health(&self) -> String;
    /// `{ network?, source }`: probes a source now ("" for every enabled server), with its certificate.
    fn probe(&self, params_json: String) -> String;
    fn exposure(&self) -> String;
    fn job_status(&self, job_id: String) -> String;
    fn send_status(&self, request_id: String) -> String;
    fn list_sends(&self) -> String;

    /// Needs the custodian role, as do the methods down to rebroadcast.
    fn set_active_network(&self, network: String) -> String;
    /// `{ network?, group?, family?, birthday?, label? }`. The reply adds `openHandle` and `next`:
    /// how the person decides the open with evm_keystore_cli.
    fn open_wallet(&self, params_json: String) -> String;
    fn close_wallet(&self) -> String;
    fn descriptors(&self) -> String;
    /// `{ date: "YYYY-MM-DD" }` or `{ height }`: the open wallet scans again from there.
    fn set_birthday(&self, params_json: String) -> String;
    fn set_sources(&self, list_json: String) -> String;
    fn set_source(&self, source_id: String, changes_json: String) -> String;
    fn set_proxy(&self, proxy: String) -> String;
    fn clear_suspect(&self, source_id: String) -> String;
    /// `{ network, peers: [host[:port]] }`, at most 64; `[]` goes back to the curated list.
    fn set_own_peers(&self, params_json: String) -> String;
    /// `{ network? }`: the peer list, with `ownPeers`.
    fn peer_list(&self, params_json: String) -> String;
    fn rebroadcast(&self, request_or_txid: String) -> String;
    /// `{ txid }` of a send in `unknown`: frees its coins; the reply's `note` says what that risks.
    fn abandon_send(&self, params_json: String) -> String;

    /// `{ recipients: [{ address, amountSat | amountBtc }], feeRate? | target?, sendMax? }`. A
    /// `prompt` event follows once the send awaits approval.
    fn request_send(&self, request_json: String) -> String;
    /// `send <address> str:<BTC>`: one recipient at the normal fee target.
    fn send(&self, address: String, amount_btc: String) -> String;
    /// `{ txid, feeRate }`: replace an unconfirmed send with a higher fee.
    fn request_fee_bump(&self, request_json: String) -> String;
    /// This module's own sends; a custodian's, any.
    fn cancel_send(&self, request_id: String) -> String;
    /// The backend's send_status plus `text`, the block a person reads.
    fn show(&self, request_id: String) -> String;

    fn on_context_ready(&self, _ctx: &RustModuleContext) {}
}

#[allow(dead_code)] // Read by logos-lidl-gen, which generates the emitters.
pub trait BitcoinWalletCliModuleEvents {
    /// A send awaits approval; `text` names the approval handle and the signer command.
    fn prompt(&self, request_id: String, text: String);
    /// A send seen open reached sent, failed, cancelled or unknown, or left the list (gone).
    fn settled(&self, request_id: String, state: String);
    /// How many sends await approval, when it changes.
    fn queue_changed(&self, count: i64);
}

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/generated/provider_gen.rs"));

use bitcoin_wallet_backend::BitcoinWalletBackendClient as Backend;

/// The backend answers within its own budgets; this leaves the host's 20 s deadline intact.
const BUDGET: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(1000);
/// The poll thread's calls give up sooner, so a vanished backend costs little.
const POLL_BUDGET: Duration = Duration::from_secs(2);

static UNLOADING: AtomicBool = AtomicBool::new(false);

/// Waits for an async reply on this thread, never in a nested event loop; unload answers at once.
fn wait<T>(rx: Receiver<Result<T, LogosError>>, budget: Duration) -> Result<T, String> {
    let start = Instant::now();
    loop {
        if UNLOADING.load(Ordering::SeqCst) {
            return Err("unloading".into());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(r) => return r.map_err(|e| e.to_string()),
            Err(RecvTimeoutError::Timeout) if start.elapsed() < budget + Duration::from_secs(1) => continue,
            Err(_) => return Err("no answer".into()),
        }
    }
}

macro_rules! call {
    ($method:ident $(, $arg:expr)* $(,)?) => {
        call_within!(BUDGET, $method $(, $arg)*)
    };
}

macro_rules! call_within {
    ($budget:expr, $method:ident $(, $arg:expr)* $(,)?) => {{
        let (tx, rx) = sync_channel(1);
        backend().$method($($arg,)* $budget, move |r| {
            let _ = tx.send(r);
        });
        wait(rx, $budget)
    }};
}

fn backend() -> &'static Backend {
    static B: OnceLock<Backend> = OnceLock::new();
    B.get_or_init(Backend::new)
}

fn err(e: impl std::fmt::Display) -> String {
    json!({"ok": false, "error": e.to_string()}).to_string()
}

fn identity() -> Value {
    call!(caller_identity_async_with_timeout).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

/// Relays a reply; a refusal names the role this method needed and the configure that fixes it.
fn relay_as(reply: Result<String, String>, role_word: &str) -> String {
    match reply {
        Ok(s) => translate_refusal(&s, ME, role_word, || fix(&identity(), ME)),
        Err(e) => err(format!("bitcoin_wallet_backend did not answer: {e}")),
    }
}

fn custodian(reply: Result<String, String>) -> String {
    relay_as(reply, "a custodian")
}

fn read(reply: Result<String, String>) -> String {
    relay_as(reply, "a custodian")
}

fn status_of(id: &str) -> Option<Value> {
    call!(send_status_async_with_timeout, id).ok().and_then(|s| serde_json::from_str(&s).ok())
}

fn poll_status_of(id: &str) -> Option<Value> {
    call_within!(POLL_BUDGET, send_status_async_with_timeout, id).ok().and_then(|s| serde_json::from_str(&s).ok())
}

#[derive(Default)]
pub struct BitcoinWalletCliModuleImpl {
    watch: std::sync::Arc<std::sync::Mutex<Watch>>,
    awaiting: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    poller: std::sync::Mutex<Option<JoinHandle<()>>>,
}

/// One poll of the backend's sends, turned into this module's events. False if it didn't answer.
fn poll(watch: &std::sync::Mutex<Watch>, awaiting: &std::sync::Mutex<Vec<String>>) -> bool {
    let Ok(raw) = call_within!(POLL_BUDGET, list_sends_async_with_timeout) else { return false };
    let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let rows = v["sends"].as_array().cloned().unwrap_or_default();
    let sends: Vec<Row> = rows
        .iter()
        .filter_map(|r| Some(Row { id: r["requestId"].as_str()?.into(), state: r["state"].as_str()?.into(), created: r["createdAt"].as_u64().unwrap_or(0) }))
        .collect();
    *awaiting.lock().unwrap_or_else(|e| e.into_inner()) =
        sends.iter().filter(|r| r.state == "awaiting_approval").map(|r| r.id.clone()).collect();
    let turn = watch.lock().unwrap_or_else(|e| e.into_inner()).turn(&sends);
    for id in turn.prompt {
        if let Some(st) = poll_status_of(&id) {
            emit_prompt(&id, &render::prompt(&st, ME));
        }
    }
    for (id, state) in turn.settled {
        emit_settled(&id, &state);
    }
    if let Some(n) = turn.awaiting {
        emit_queue_changed(n as i64);
    }
    true
}

impl BitcoinWalletCliModule for BitcoinWalletCliModuleImpl {
    fn status(&self) -> String {
        let id = identity();
        let held = enrolled(&id, ME);
        let awaiting = self.awaiting.lock().unwrap_or_else(|e| e.into_inner()).clone();
        json!({
            "ok": true, "enrolled": held, "identity": id["identity"], "custodians": id["custodians"],
            "rolesFile": id["rolesFile"], "awaiting": awaiting,
            "fix": if held { Value::Null } else { Value::String(fix(&id, ME)) },
        })
        .to_string()
    }

    fn caller_identity(&self) -> String {
        read(call!(caller_identity_async_with_timeout))
    }
    fn list_networks(&self) -> String {
        read(call!(list_networks_async_with_timeout))
    }
    fn list_wallets(&self) -> String {
        read(call!(list_wallets_async_with_timeout))
    }
    fn wallet_status(&self) -> String {
        read(call!(wallet_status_async_with_timeout))
    }
    fn balances(&self) -> String {
        read(call!(balances_async_with_timeout))
    }
    fn history(&self, page: i64) -> String {
        read(call!(history_async_with_timeout, page))
    }
    fn receive_address(&self) -> String {
        read(call!(receive_address_async_with_timeout))
    }
    fn fee_rates(&self) -> String {
        read(call!(fee_rates_async_with_timeout))
    }
    fn address_valid(&self, text: String) -> String {
        read(call!(address_valid_async_with_timeout, text.trim()))
    }
    fn format_btc(&self, sat: String) -> String {
        read(call!(format_btc_async_with_timeout, sat.trim()))
    }
    fn parse_btc(&self, btc: String) -> String {
        read(call!(parse_btc_async_with_timeout, btc.trim()))
    }
    fn sources(&self) -> String {
        read(call!(sources_async_with_timeout))
    }
    fn source_health(&self) -> String {
        read(call!(source_health_async_with_timeout))
    }
    fn probe(&self, params_json: String) -> String {
        read(call!(probe_async_with_timeout, &params_json))
    }
    fn exposure(&self) -> String {
        read(call!(exposure_async_with_timeout))
    }
    fn job_status(&self, job_id: String) -> String {
        read(call!(job_status_async_with_timeout, job_id.trim()))
    }
    fn send_status(&self, request_id: String) -> String {
        read(call!(send_status_async_with_timeout, request_id.trim()))
    }
    fn list_sends(&self) -> String {
        read(call!(list_sends_async_with_timeout))
    }

    fn set_active_network(&self, network: String) -> String {
        custodian(call!(set_active_network_async_with_timeout, network.trim()))
    }

    fn open_wallet(&self, params_json: String) -> String {
        let reply = custodian(call!(open_wallet_async_with_timeout, &params_json));
        let mut v: Value = match serde_json::from_str(&reply) {
            Ok(v) => v,
            Err(_) => return reply,
        };
        if v["ok"] != true {
            return reply;
        }
        // The core asks the signer manager first; its handle shows within a moment.
        let mut handle = None;
        for _ in 0..8 {
            let st: Value = call!(wallet_status_async_with_timeout).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
            if let Some(h) = st["openHandle"].as_str() {
                handle = Some(h.to_string());
                break;
            }
            if matches!(st["state"].as_str(), Some("opening" | "syncing" | "ready" | "failed")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        v["openHandle"] = handle.as_ref().map(|h| json!(h)).unwrap_or(Value::Null);
        v["next"] = json!(render::open_next(handle.as_deref()));
        v.to_string()
    }

    fn close_wallet(&self) -> String {
        custodian(call!(close_wallet_async_with_timeout))
    }
    fn descriptors(&self) -> String {
        custodian(call!(descriptors_async_with_timeout))
    }
    fn set_birthday(&self, params_json: String) -> String {
        custodian(call!(set_birthday_async_with_timeout, &params_json))
    }
    fn set_sources(&self, list_json: String) -> String {
        custodian(call!(set_sources_async_with_timeout, &list_json))
    }
    fn set_source(&self, source_id: String, changes_json: String) -> String {
        custodian(call!(set_source_async_with_timeout, source_id.trim(), &changes_json))
    }
    fn set_proxy(&self, proxy: String) -> String {
        custodian(call!(set_proxy_async_with_timeout, proxy.trim()))
    }
    fn clear_suspect(&self, source_id: String) -> String {
        custodian(call!(clear_suspect_async_with_timeout, source_id.trim()))
    }
    fn set_own_peers(&self, params_json: String) -> String {
        custodian(call!(set_own_peers_async_with_timeout, &params_json))
    }
    fn peer_list(&self, params_json: String) -> String {
        custodian(call!(peer_list_async_with_timeout, &params_json))
    }
    fn rebroadcast(&self, request_or_txid: String) -> String {
        custodian(call!(rebroadcast_async_with_timeout, request_or_txid.trim()))
    }
    fn abandon_send(&self, params_json: String) -> String {
        custodian(call!(abandon_send_async_with_timeout, &params_json))
    }

    fn request_send(&self, request_json: String) -> String {
        read(call!(request_send_async_with_timeout, &request_json))
    }

    fn send(&self, address: String, amount_btc: String) -> String {
        let doc = json!({"recipients": [{"address": address.trim(), "amountBtc": amount_btc.trim()}], "target": "normal"}).to_string();
        read(call!(request_send_async_with_timeout, &doc))
    }

    fn request_fee_bump(&self, request_json: String) -> String {
        read(call!(request_fee_bump_async_with_timeout, &request_json))
    }

    fn cancel_send(&self, request_id: String) -> String {
        relay_as(call!(cancel_send_async_with_timeout, request_id.trim()), "the requester or a custodian")
    }

    fn show(&self, request_id: String) -> String {
        match status_of(request_id.trim()) {
            Some(mut st) if st["ok"] == true => {
                st["text"] = json!(render::show(&st, ME));
                st.to_string()
            }
            Some(st) => st.to_string(),
            None => err("bitcoin_wallet_backend did not answer"),
        }
    }

    fn on_context_ready(&self, _ctx: &RustModuleContext) {
        let (watch, awaiting) = (self.watch.clone(), self.awaiting.clone());
        // Nothing outbound here: the poll thread's first call is a second away.
        let handle = std::thread::spawn(move || {
            let mut pause = None;
            while !UNLOADING.load(Ordering::SeqCst) {
                // Each poll that gets no answer waits longer: 100 ms, doubling to 5 s.
                let until = Instant::now() + pause.unwrap_or(POLL);
                while Instant::now() < until && !UNLOADING.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                if UNLOADING.load(Ordering::SeqCst) {
                    break;
                }
                pause = crate::watch::backoff(pause, !poll(&watch, &awaiting));
            }
        });
        *self.poller.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
    }
}

impl AboutToUnload for BitcoinWalletCliModuleImpl {
    /// Stops the poll thread; a call in flight answers at once, so it joins quickly.
    fn about_to_unload(&self) -> Shutdown {
        UNLOADING.store(true, Ordering::SeqCst);
        if let Some(t) = self.poller.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let until = Instant::now() + Duration::from_millis(1500);
            while !t.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
        Shutdown::Synchronous
    }
}

#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    logos_install!(BitcoinWalletCliModuleImpl);
}
