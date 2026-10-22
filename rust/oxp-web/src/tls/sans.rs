// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-21

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Name {
    Dns(String),
    Ip(IpAddr),
}
impl Name {
    pub(super) fn from_text(name: &str) -> Self {
        match name.parse() {
            Ok(ip) => Self::Ip(ip),
            Err(_) => Self::Dns(normalize_dns(name)),
        }
    }
    pub(super) fn normalized(&self) -> Self {
        match self {
            Self::Dns(name) => Self::Dns(normalize_dns(name)),
            Self::Ip(ip) => Self::Ip(*ip),
        }
    }
    pub(super) fn is_ip(&self) -> bool {
        matches!(self, Self::Ip(_))
    }
}
impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dns(name) => name.fmt(f),
            Self::Ip(ip) => ip.fmt(f),
        }
    }
}
fn normalize_dns(name: &str) -> String {
    name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase()
}

// Only read the X.509 structure needed to find subjectAltName. Rustls validates the identity.
pub(super) fn parse(certificate: &[u8]) -> Result<Vec<Name>> {
    let mut certificate = sequence(certificate)?;
    let mut tbs = take(&mut certificate, 0x30)?;
    take(&mut certificate, 0x30)?; // Signature algorithm.
    take(&mut certificate, 0x03)?; // Signature value.
    empty(certificate)?;
    if tbs.first() == Some(&0xa0) {
        take(&mut tbs, 0xa0)?; // Explicit version.
    }
    take(&mut tbs, 0x02)?; // Serial number.
    for _ in 0..5 {
        // Signature algorithm, issuer, validity, subject and subject public key info.
        take(&mut tbs, 0x30)?;
    }
    for tag in [0x81, 0x82] {
        if tbs.first() == Some(&tag) {
            take(&mut tbs, tag)?; // Optional issuer/subject unique identifiers.
        }
    }
    if tbs.is_empty() {
        return Ok(Vec::new());
    }
    let mut extensions = sequence(take(&mut tbs, 0xa3)?)?;
    empty(tbs)?;
    let mut names = None;
    while !extensions.is_empty() {
        let mut extension = take(&mut extensions, 0x30)?;
        let oid = take(&mut extension, 0x06)?;
        if extension.first() == Some(&0x01) {
            let critical = take(&mut extension, 0x01)?;
            if critical != [0] && critical != [0xff] {
                return Err("Invalid extension critical flag");
            }
        }
        let value = take(&mut extension, 0x04)?;
        empty(extension)?;
        if oid == [0x55, 0x1d, 0x11] {
            if names.is_some() {
                return Err("Duplicate subjectAltName extension");
            }
            names = Some(general_names(value)?);
        }
    }
    Ok(names.unwrap_or_default())
}

fn general_names(value: &[u8]) -> Result<Vec<Name>> {
    let mut value = sequence(value)?;
    let mut names = Vec::new();
    while !value.is_empty() {
        let (tag, name) = element(&mut value)?;
        match tag {
            0x82 if !name.is_empty() && name.is_ascii() => {
                names.push(Name::Dns(
                    std::str::from_utf8(name)
                        .map_err(|_| "Invalid DNS SAN")?
                        .into(),
                ));
            }
            0x87 => {
                let ip = match name.len() {
                    4 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(name).unwrap())),
                    16 => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(name).unwrap())),
                    _ => return Err("Invalid IP SAN length"),
                };
                names.push(Name::Ip(ip));
            }
            // Other GeneralName choices do not name browser hosts.
            0xa0 | 0x81 | 0xa3 | 0xa4 | 0xa5 | 0x86 | 0x88 => {}
            _ => return Err("Invalid subjectAltName entry"),
        }
    }
    Ok(names)
}

fn sequence(value: &[u8]) -> Result<&[u8]> {
    let mut value = value;
    let inner = take(&mut value, 0x30)?;
    empty(value)?;
    Ok(inner)
}

fn empty(value: &[u8]) -> Result<()> {
    if value.is_empty() {
        Ok(())
    } else {
        Err("Unexpected trailing DER data")
    }
}

fn take<'a>(value: &mut &'a [u8], expected: u8) -> Result<&'a [u8]> {
    let (tag, inner) = element(value)?;
    if tag != expected {
        return Err("Unexpected DER tag");
    }
    Ok(inner)
}

fn element<'a>(value: &mut &'a [u8]) -> Result<(u8, &'a [u8])> {
    let (&tag, rest) = value.split_first().ok_or("Missing DER tag")?;
    if tag & 0x1f == 0x1f {
        return Err("Unsupported DER tag");
    }
    let (&length, mut rest) = rest.split_first().ok_or("Missing DER length")?;
    let length = if length & 0x80 == 0 {
        usize::from(length)
    } else {
        let count = usize::from(length & 0x7f);
        if count == 0 || count > std::mem::size_of::<usize>() {
            return Err("Invalid DER length");
        }
        let bytes = rest.get(..count).ok_or("Truncated DER length")?;
        if bytes[0] == 0 {
            return Err("Nonminimal DER length");
        }
        let mut length = 0usize;
        for byte in bytes {
            length = length
                .checked_mul(256)
                .and_then(|n| n.checked_add(usize::from(*byte)))
                .ok_or("DER length overflow")?;
        }
        if length < 128 {
            return Err("Nonminimal DER length");
        }
        rest = &rest[count..];
        length
    };
    let inner = rest.get(..length).ok_or("Truncated DER value")?;
    *value = &rest[length..];
    Ok((tag, inner))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_dns_ipv4_ipv6_and_ignores_other_names_and_extensions() {
        let expected = ["Relay.Local", "127.0.0.1", "2001:db8::1"];
        let mut params = rcgen::CertificateParams::new(expected.map(String::from)).unwrap();
        params.subject_alt_names.extend([
            rcgen::SanType::Rfc822Name("relay@example.test".try_into().unwrap()),
            rcgen::SanType::URI("https://example.test".try_into().unwrap()),
        ]);
        params
            .custom_extensions
            .push(rcgen::CustomExtension::from_oid_content(
                &[1, 2, 3, 4],
                vec![0x05, 0],
            ));
        let cert = params
            .self_signed(&rcgen::KeyPair::generate().unwrap())
            .unwrap();
        let actual: Vec<_> = parse(cert.der().as_ref())
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(actual, expected);
        let no_sans = rcgen::generate_simple_self_signed(Vec::new()).unwrap();
        assert!(parse(no_sans.cert.der().as_ref()).unwrap().is_empty());
    }

    #[test]
    fn rejects_truncated_certificates_and_malformed_san_extensions() {
        let cert = rcgen::generate_simple_self_signed(vec!["relay.test".into()]).unwrap();
        let der = cert.cert.der().as_ref();
        for end in 0..der.len() {
            assert!(parse(&der[..end]).is_err(), "truncation at {end}");
        }
        let mut trailing = der.to_vec();
        trailing.push(0);
        assert!(parse(&trailing).is_err());
        for value in [
            vec![],
            vec![0x30, 0x80, 0, 0],
            vec![0x30, 3, 0x87, 1, 127],
            vec![0x30, 3, 0x82, 1, 0xff],
            vec![0x30, 2, 0x82, 0],
            vec![0x30, 2, 0x85, 0],
        ] {
            let mut params = rcgen::CertificateParams::default();
            params
                .custom_extensions
                .push(rcgen::CustomExtension::from_oid_content(
                    &[2, 5, 29, 17],
                    value,
                ));
            let cert = params
                .self_signed(&rcgen::KeyPair::generate().unwrap())
                .unwrap();
            assert!(parse(cert.der().as_ref()).is_err());
        }
        let mut params = rcgen::CertificateParams::new(vec!["relay.test".into()]).unwrap();
        params
            .custom_extensions
            .push(rcgen::CustomExtension::from_oid_content(
                &[2, 5, 29, 17],
                vec![0x30, 0],
            ));
        let cert = params
            .self_signed(&rcgen::KeyPair::generate().unwrap())
            .unwrap();
        assert!(parse(cert.der().as_ref()).is_err());
    }

    #[test]
    fn rejects_bad_der_lengths_without_panicking() {
        for der in [
            vec![],
            vec![0x30],
            vec![0x30, 0x80],
            vec![0x30, 0x81],
            vec![0x30, 0x81, 0x7f],
            vec![0x30, 0x82, 0, 0x80],
            vec![0x30, 0x89, 1, 0, 0, 0, 0, 0, 0, 0, 0],
            vec![0x30, 0x88, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            vec![0x1f, 0],
            vec![0x30, 2, 0],
        ] {
            assert!(element(&mut der.as_slice()).is_err(), "{der:?}");
        }
        let mut der = vec![0x30, 0x81, 0x80];
        der.extend([0; 128]);
        assert_eq!(element(&mut der.as_slice()).unwrap(), (0x30, &[0; 128][..]));
    }
}
