# logos-bitcoin-wallet-cli

`bitcoin_wallet_cli`: headless Bitcoin wallet sessions for `logosctl`. It relays to
`bitcoin_wallet_backend` as itself, because the backend refuses `logosctl`'s own host
identity. It never handles a password: a person unlocks the wallet and allows an open with
`evm_signer_cli`, and approves a send there too.

## Enrolling it

Reads and send requests need no role. Opening and closing a wallet, the network, the source
settings, descriptors and rebroadcasts need the backend's custodian role. A refusal comes
back as the exact `configure` document that admits this module, keeping the custodians
already in force, and `status` says the same:

```text
not authorized: bitcoin_wallet_cli is not a custodian of bitcoin_wallet_backend. The fix:
configure '{"custodians":["bitcoin_wallet_ui","bitcoin_wallet_cli"]}' — a custodian
(bitcoin_wallet_ui) calls it; headless, write that document to <…>/roles.json, then:
logosctl module reload bitcoin_wallet_backend
```

The reload keeps this module loaded; unloading the backend would unload it too.

## A session

```bash
logosctl module load bitcoin_wallet_cli
logosctl call bitcoin_wallet_cli status
logosctl call bitcoin_wallet_cli set_active_network signet
logosctl call bitcoin_wallet_cli open_wallet '{"family":"bitcoin"}'
# → { ok, jobId, openHandle, next }: unlock the wallet and allow it with evm_signer_cli, as `next` says
logosctl call bitcoin_wallet_cli wallet_status
logosctl call bitcoin_wallet_cli receive_address
logosctl call bitcoin_wallet_cli balances

logosctl watch bitcoin_wallet_cli --event prompt &
logosctl call bitcoin_wallet_cli send tb1q… str:0.0006
# a prompt names the approval handle; approve it in the signer:
logosctl call evm_signer_cli show smh_…
logosctl call evm_signer_cli approve smh_… <bundle id> @/path/to/pwfile
logosctl call bitcoin_wallet_cli send_status s1
```

`send <address> <BTC>` asks for one payment at the normal fee target; `request_send` takes the
backend's whole document, `smallChangeToFee` included, and `show` says when change went to the fee.
`set_birthday '{"height":…}'` (or `{"date":"YYYY-MM-DD"}`) gives a restored wallet a birthday,
`set_gap_limit '{"gapLimit":100}'` (20 to 1000; `open_wallet` takes `gapLimit` too) sets how far past
the last used address the wallet looks, and `status` shows `beyondGap`: coins found past that
limit, kept out of the balance but still spendable,
`probe '{"source":"…"}'` checks a source now, with its certificate, and
`set_own_peers '{"network":"signet","peers":["…"]}'` and `peer_list` manage Kyoto's own peers. Amounts that look like numbers need `str:`, or `logosctl` passes a
number and the call is refused.

## Events

| Event | Says |
|---|---|
| `prompt(requestId, text)` | a send awaits approval: what it pays, the change, the fee, and the `evm_signer_cli` commands that show, approve or reject it, with the handle filled in |
| `settled(requestId, state)` | a send seen open reached `sent`, `failed`, `cancelled` or `unknown` (`gone` if it left the list) |
| `queue_changed(count)` | how many sends await approval |

They come from polling `list_sends` once a second, so a send another module asked for is
announced too. A poll the backend doesn't answer waits 100 ms, doubling to 5 s, and polling stops
once unload begins. An offer nobody acknowledges within 60 s expires; request the send again. A
send in `unknown` may have left: `show` offers `rebroadcast`, `request_fee_bump` and
`abandon_send '{"txid":"…"}'`, which frees its coins (custodian only).

## Build and test

```bash
cd rust-lib && cargo test --no-default-features
nix build .#lgx
```

The backend's contract is committed under `contracts/` and named by `dependency_overrides` in
`metadata.json`, so the module builds without the backend's sources. Regenerate it from the
backend's `glue.rs` with `logos-lidl-gen --from-rust` when the backend's methods change.
