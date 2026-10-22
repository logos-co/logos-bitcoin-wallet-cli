//! bitcoin_wallet_cli: headless Bitcoin wallet sessions for logosctl, relayed to
//! bitcoin_wallet_backend. It never handles a password.

pub mod relay;
pub mod render;
pub mod watch;

#[cfg(feature = "logos_module")]
mod glue;

/// No method takes a secret: unlocks, opens and approvals go through evm_signer_cli.
#[cfg(test)]
mod contract {
    const BANNED: [&str; 12] = ["password", "passphrase", "phrase", "mnemonic", "seed", "secret", "key", "xprv", "tprv", "privkey", "wif", "pin"];

    fn params(src: &str, name: &str) -> Vec<(String, String)> {
        let start = src.find(&format!("pub trait {name}")).expect("the trait");
        let end = start + src[start..].find("\n}\n").expect("the trait's end");
        let mut out = vec![];
        for line in src[start..end].lines().map(str::trim).filter(|l| l.starts_with("fn ")) {
            let method = line[3..line.find('(').unwrap()].to_string();
            let args = &line[line.find('(').unwrap() + 1..line.rfind(')').unwrap()];
            for a in args.split(',').map(str::trim).filter(|a| !a.is_empty() && !a.contains("self")) {
                out.push((method.clone(), a.split(':').next().unwrap().trim().to_string()));
            }
        }
        out
    }

    #[test]
    fn no_method_takes_a_secret() {
        let glue = include_str!("glue.rs");
        let all: Vec<(String, String)> = [params(glue, "BitcoinWalletCliModule:"), params(glue, "BitcoinWalletCliModuleEvents")].concat();
        assert!(all.len() >= 15, "{all:?}");
        for (m, p) in &all {
            let p = p.to_ascii_lowercase();
            assert!(!BANNED.iter().any(|b| p.contains(b)), "{m}({p}) is named like a secret");
        }
        let fake = "pub trait BitcoinWalletCliModule: Send {\n    fn approve(&self, handle: String, password: String) -> String;\n}\n";
        assert_eq!(params(fake, "BitcoinWalletCliModule:")[1].1, "password", "the check reads parameters");
    }
}
