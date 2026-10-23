// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use oxp_wire::Result;
use rustls::{
    crypto::ring,
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
    ServerConfig,
};
use rustls_pki_types::{pem::PemObject, CertificateDer, DnsName, PrivateKeyDer};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
mod sans;
use sans::Name;
#[cfg(test)]
mod tests;

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
    Sni {
        name: String,
        cert: PathBuf,
        key: PathBuf,
        state_dir: PathBuf,
        sans: Vec<String>,
    },
}
pub struct Identity {
    pub config: Arc<ServerConfig>,
    pub fingerprint: String,
    pub trusted_name: Option<String>,
    pub trusted_fingerprint: Option<String>,
}
impl Identity {
    pub(crate) fn fingerprint_event(&self) -> Value {
        let mut event = json!({
            "type": "tls-fingerprint", "algorithm": "SHA-256", "fingerprint": self.fingerprint
        });
        if let Some(name) = &self.trusted_name {
            event["trustedName"] = json!(name);
            event["trustedFingerprint"] = json!(self.trusted_fingerprint);
        }
        event
    }
}

/// Whether a name can select a trusted certificate through SNI (DNS LDH labels only).
pub fn valid_dns_name(name: &str) -> bool {
    let name = name.strip_suffix('.').unwrap_or(name);
    !name.ends_with('.')
        && DnsName::try_from(name).is_ok()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

#[derive(Debug)]
struct SniResolver {
    name: Option<String>,
    trusted: Option<Arc<CertifiedKey>>,
    fallback: Arc<CertifiedKey>,
}
impl SniResolver {
    fn select(&self, server_name: Option<&str>) -> Arc<CertifiedKey> {
        if let (Some(sni), Some(name), Some(trusted)) = (server_name, &self.name, &self.trusted) {
            if sni
                .strip_suffix('.')
                .unwrap_or(sni)
                .eq_ignore_ascii_case(name)
            {
                return trusted.clone();
            }
        }
        self.fallback.clone()
    }
}
impl ResolvesServerCert for SniResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.select(client_hello.server_name()))
    }
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
    let (fallback, trusted_name, trusted) = match tls {
        Tls::Disabled => return Ok(None),
        Tls::Pem { cert, key } => (certified_key(cert, key)?, None, None),
        Tls::SelfSigned { state_dir, sans } => (self_signed(state_dir, sans)?, None, None),
        Tls::Sni {
            name,
            cert,
            key,
            state_dir,
            sans,
        } => {
            if !valid_dns_name(name) {
                return Err("Invalid TLS SNI DNS name".into());
            }
            let fallback = self_signed(state_dir, sans)?;
            let trusted = match certified_key(cert, key) {
                Ok(key) => Some(key),
                Err(e) => {
                    eprintln!("Could not load trusted TLS identity for {name}; using self-signed TLS: {e}");
                    None
                }
            };
            let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
            (fallback, Some(name), trusted)
        }
    };
    let trusted_fingerprint = trusted.as_ref().map(|key| fingerprint(key));
    let fingerprint = fingerprint(&fallback);
    let config = ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(SniResolver {
            name: trusted_name.clone(),
            trusted,
            fallback,
        }));
    Ok(Some(Identity {
        config: Arc::new(config),
        fingerprint,
        trusted_name,
        trusted_fingerprint,
    }))
}

fn certified_key(cert: &Path, key: &Path) -> Result<Arc<CertifiedKey>> {
    let cert_bytes = fs::read(cert)?;
    let certs: Vec<_> =
        CertificateDer::pem_slice_iter(&cert_bytes).collect::<std::result::Result<_, _>>()?;
    for cert in &certs {
        rustls::server::ParsedCertificate::try_from(cert)?;
    }
    let key = PrivateKeyDer::from_pem_file(key)?;
    Ok(Arc::new(CertifiedKey::from_der(
        certs,
        key,
        &ring::default_provider(),
    )?))
}

fn fingerprint(key: &CertifiedKey) -> String {
    let digest = ring::cipher_suite::TLS13_AES_128_GCM_SHA256
        .tls13()
        .unwrap()
        .common
        .hash_provider
        .hash(key.cert[0].as_ref());
    digest
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn self_signed(state_dir: &Path, sans: &[String]) -> Result<Arc<CertifiedKey>> {
    let mut required = vec!["localhost".into(), "127.0.0.1".into()];
    required.extend(local_ipv4());
    required.extend_from_slice(sans);
    let (cert, key) = self_signed_paths(state_dir, &required)?;
    certified_key(&cert, &key)
}

fn self_signed_paths(state_dir: &Path, required: &[String]) -> Result<(PathBuf, PathBuf)> {
    fs::create_dir_all(state_dir)?;
    let cert = state_dir.join("cert.pem");
    let key = state_dir.join("key.pem");
    let names = match (cert.exists(), key.exists()) {
        (true, true) => {
            // Only an unusable pair is replaced; a usable one whose SANs this parser cannot read is
            // kept, so a parser gap never costs the head unit a new certificate warning per start.
            match certified_key(&cert, &key) {
                Ok(identity) => match sans::parse(identity.cert[0].as_ref()) {
                    Ok(old) => accumulate_sans(&old, required),
                    Err(e) => {
                        eprintln!("Could not read self-signed TLS SANs; keeping it unchanged: {e}");
                        None
                    }
                },
                Err(e) => {
                    eprintln!(
                        "Could not load existing self-signed TLS identity; regenerating: {e}"
                    );
                    Some(merge_sans(&[], required))
                }
            }
        }
        (false, false) => Some(merge_sans(&[], required)),
        _ => {
            eprintln!("Incomplete self-signed TLS identity (cert.pem or key.pem is missing); regenerating");
            Some(merge_sans(&[], required))
        }
    };
    if let Some(names) = names {
        let mut params = rcgen::CertificateParams::new(Vec::new())?;
        params.subject_alt_names = names
            .into_iter()
            .map(|name| {
                Ok(match name {
                    Name::Dns(name) => rcgen::SanType::DnsName(name.try_into()?),
                    Name::Ip(ip) => rcgen::SanType::IpAddress(ip),
                })
            })
            .collect::<Result<_>>()?;
        let key_pair = rcgen::KeyPair::generate()?;
        let certificate = params.self_signed(&key_pair)?;
        let staged_key = StagedFile::new(&key, key_pair.serialize_pem().as_bytes(), 0o600)?;
        let staged_cert = StagedFile::new(&cert, certificate.pem().as_bytes(), 0o644)?;
        // Publish the certificate last; interrupted replacements are regenerated on the next load.
        fs::rename(&staged_key.0, &key)?;
        fs::rename(&staged_cert.0, &cert)?;
    }
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600))?;
    Ok((cert, key))
}

fn accumulate_sans(old: &[Name], required: &[String]) -> Option<Vec<Name>> {
    let old: Vec<_> = old.iter().map(Name::normalized).collect();
    if required
        .iter()
        .all(|name| old.contains(&Name::from_text(name)))
    {
        return None;
    }
    Some(merge_sans(&old, required))
}

fn merge_sans(old: &[Name], required: &[String]) -> Vec<Name> {
    let required: Vec<_> = required.iter().map(|name| Name::from_text(name)).collect();
    let mut names = Vec::new();
    // Certificate order records age; append new names and evict the oldest optional ones first.
    for name in old
        .iter()
        .map(Name::normalized)
        .chain(required.iter().cloned())
    {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    for (ip, limit) in [(false, 8), (true, 16)] {
        let mut excess = names
            .iter()
            .filter(|name| name.is_ip() == ip)
            .count()
            .saturating_sub(limit);
        names.retain(|name| {
            if excess > 0 && name.is_ip() == ip && !required.contains(name) {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }
    names
}

struct StagedFile(PathBuf);
impl StagedFile {
    fn new(path: &Path, contents: &[u8], mode: u32) -> Result<Self> {
        let temporary = path.with_extension(format!(
            "pem.{}.{}.tmp",
            std::process::id(),
            random_token()?
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        let staged = Self(temporary);
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.write_all(contents)?;
        file.sync_all()?;
        Ok(staged)
    }
}
impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
