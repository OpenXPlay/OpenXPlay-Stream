// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use oxp_wire::Result;
use rustls::{crypto::ring, ServerConfig};
use rustls_pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::Arc,
};
#[derive(Clone, Debug)]
pub enum Tls {
    Disabled,
    Pem {
        cert: PathBuf,
        key: PathBuf,
    },
    SelfSigned {
        state_dir: PathBuf,
        sans: Vec<String>,
    },
}
pub struct Identity {
    pub config: Arc<ServerConfig>,
    pub fingerprint: String,
}
/// Generates an eight-digit pairing code with the same cryptographic provider as TLS.
pub fn random_token() -> Result<String> {
    const RANGE: u32 = 100_000_000;
    // Reject the incomplete final range so every code is equally likely.
    const LIMIT: u32 = u32::MAX - u32::MAX % RANGE;
    loop {
        let mut bytes = [0; 4];
        ring::default_provider()
            .secure_random
            .fill(&mut bytes)
            .map_err(|_| "Random generator failed")?;
        let value = u32::from_ne_bytes(bytes);
        if value < LIMIT {
            return Ok(format!("{:08}", value % RANGE));
        }
    }
}
/// Local IPv4 addresses (no loopback), as named in the self-signed certificate.
pub fn local_ipv4() -> Vec<String> {
    let mut result = Vec::new();
    // Linux exposes local IPv4 routes here even when iproute2 is not installed.
    if let Ok(routes) = fs::read_to_string("/proc/net/fib_trie") {
        let mut address = None;
        for line in routes.lines() {
            for word in line.split_whitespace() {
                if let Ok(ip) = word.parse::<std::net::Ipv4Addr>() {
                    address = Some(ip);
                }
            }
            if line.contains("/32 host LOCAL") {
                if let Some(ip) = address {
                    if !ip.is_loopback() && !ip.is_unspecified() {
                        result.push(ip.to_string());
                    }
                }
            }
        }
        result.sort();
        result.dedup();
        return result;
    }
    // Development hosts use their local interface inventory; no remote host is contacted.
    let commands: [(&str, &[&str]); 2] = [("ip", &["-o", "-4", "addr", "show"]), ("ifconfig", &[])];
    for (program, args) in commands {
        if let Ok(out) = std::process::Command::new(program).args(args).output() {
            if !out.status.success() {
                continue;
            }
            let text = String::from_utf8_lossy(&out.stdout);
            let words: Vec<_> = text.split_whitespace().collect();
            for w in words.windows(2) {
                if w[0] != "inet" {
                    continue;
                }
                if let Ok(ip) = w[1]
                    .split('/')
                    .next()
                    .unwrap_or("")
                    .parse::<std::net::Ipv4Addr>()
                {
                    if !ip.is_loopback() && !ip.is_unspecified() {
                        result.push(ip.to_string());
                    }
                }
            }
            break;
        }
    }
    result
}
pub fn load(tls: &Tls) -> Result<Option<Identity>> {
    let (cert, key) = match tls {
        Tls::Disabled => return Ok(None),
        Tls::Pem { cert, key } => (cert.clone(), key.clone()),
        Tls::SelfSigned { state_dir, sans } => {
            fs::create_dir_all(state_dir)?;
            let cert = state_dir.join("cert.pem");
            let key = state_dir.join("key.pem");
            if cert.exists() != key.exists() {
                return Err("Incomplete TLS identity; restore both cert.pem and key.pem".into());
            }
            if !cert.exists() {
                let mut names = vec!["localhost".into(), "127.0.0.1".into()];
                names.extend(local_ipv4());
                names.extend(sans.iter().cloned());
                names.sort();
                names.dedup();
                let identity = rcgen::generate_simple_self_signed(names)?;
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&key)?;
                file.write_all(identity.key_pair.serialize_pem().as_bytes())?;
                file.sync_all()?;
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&cert)?;
                file.write_all(identity.cert.pem().as_bytes())?;
                file.sync_all()?;
            }
            fs::set_permissions(&key, fs::Permissions::from_mode(0o600))?;
            (cert, key)
        }
    };
    let cert_bytes = fs::read(cert)?;
    let certs: Vec<_> =
        CertificateDer::pem_slice_iter(&cert_bytes).collect::<std::result::Result<_, _>>()?;
    let leaf = certs.first().ok_or("Empty TLS certificate chain")?;
    let digest = ring::cipher_suite::TLS13_AES_128_GCM_SHA256
        .tls13()
        .unwrap()
        .common
        .hash_provider
        .hash(leaf.as_ref());
    let fingerprint = digest
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    let key = PrivateKeyDer::from_pem_file(key)?;
    let config = ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(Some(Identity {
        config: Arc::new(config),
        fingerprint,
    }))
}
