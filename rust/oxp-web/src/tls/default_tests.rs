// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

use super::{tests::*, *};

fn imported(temp: &Temp) -> (DefaultPem, CertificateDer<'static>) {
    let generated =
        rcgen::generate_simple_self_signed(names(&["relay.test", "other.test", "127.0.0.1"]))
            .unwrap();
    let pem = DefaultPem {
        cert: temp.0.join("user.pem"),
        key: temp.0.join("user-key.pem"),
    };
    fs::write(&pem.cert, generated.cert.pem()).unwrap();
    fs::write(&pem.key, generated.key_pair.serialize_pem()).unwrap();
    (pem, generated.cert.der().clone())
}

#[test]
fn startup_user_default_is_independent_of_trusted_sni() {
    let temp = Temp::new();
    let (
        Tls::Sni {
            cert,
            key,
            state_dir,
            ..
        },
        trusted_cert,
        _,
    ) = sni_identity(&temp)
    else {
        unreachable!()
    };
    let (pem, user_cert) = imported(&temp);
    let trusted = TrustedPem {
        name: "ReLaY.TeSt.".into(),
        cert,
        key,
    };
    for trusted in [Some(trusted), None] {
        let identity = load(&Tls::DefaultPem {
            pem: pem.clone(),
            trusted: trusted.clone(),
            state_dir: state_dir.clone(),
            sans: vec![],
        })
        .unwrap()
        .unwrap();
        let event = identity.fingerprint_event();
        assert_eq!(
            event["defaultFingerprint"],
            fingerprint(&certified_key(&pem.cert, &pem.key).unwrap())
        );
        assert_eq!(event["fingerprint"], event["defaultFingerprint"]);
        assert_eq!(event["trustedFingerprint"].is_string(), trusted.is_some());
        for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
            for (name, sni) in [
                ("relay.test", true),
                ("RELAY.TEST.", true),
                ("127.0.0.1", true),
                ("other.test", true),
                ("relay.test", false),
            ] {
                let expected = if sni
                    && name.to_ascii_lowercase().starts_with("relay.test")
                    && trusted.is_some()
                {
                    &trusted_cert
                } else {
                    &user_cert
                };
                let served = handshake(
                    identity.config.clone(),
                    &[user_cert.clone(), trusted_cert.clone()],
                    name,
                    sni,
                    version,
                );
                assert_eq!(&served[0], expected, "{name}, SNI={sni}");
            }
        }
    }
}

#[test]
fn invalid_user_default_fails_startup_instead_of_falling_back() {
    let temp = Temp::new();
    let (pem, _) = imported(&temp);
    let state_dir = temp.0.join("unused");
    let config = Tls::DefaultPem {
        pem: pem.clone(),
        trusted: None,
        state_dir: state_dir.clone(),
        sans: vec![],
    };
    fs::write(
        &pem.key,
        rcgen::KeyPair::generate().unwrap().serialize_pem(),
    )
    .unwrap();
    assert!(load(&config).is_err());
    fs::write(&pem.cert, "invalid PEM").unwrap();
    assert!(load(&config).is_err());
    assert!(!state_dir.exists());
}

#[test]
fn malformed_or_empty_chain_is_rejected_even_with_a_valid_leaf_and_key() {
    let temp = Temp::new();
    let (pem, _) = imported(&temp);
    let leaf = fs::read_to_string(&pem.cert).unwrap();
    for contents in [
        String::new(),
        format!("{leaf}-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n"),
        format!("{leaf}-----BEGIN CERTIFICATE-----\n!invalid!\n-----END CERTIFICATE-----\n"),
    ] {
        fs::write(&pem.cert, contents).unwrap();
        assert!(certified_key(&pem.cert, &pem.key).is_err());
    }
}

#[test]
fn supported_ec_and_ed25519_keys_serve_the_complete_chain() {
    let temp = Temp::new();
    let issuer_key = rcgen::KeyPair::generate().unwrap();
    let mut issuer_params = rcgen::CertificateParams::new(vec![]).unwrap();
    issuer_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let issuer = issuer_params.self_signed(&issuer_key).unwrap();
    for algorithm in [
        &rcgen::PKCS_ECDSA_P256_SHA256,
        &rcgen::PKCS_ECDSA_P384_SHA384,
        &rcgen::PKCS_ED25519,
    ] {
        let key = rcgen::KeyPair::generate_for(algorithm).unwrap();
        let leaf = rcgen::CertificateParams::new(names(&["127.0.0.1"]))
            .unwrap()
            .signed_by(&key, &issuer, &issuer_key)
            .unwrap();
        let pem = DefaultPem {
            cert: temp.0.join("chain.pem"),
            key: temp.0.join("key.pem"),
        };
        fs::write(&pem.cert, format!("{}{}", leaf.pem(), issuer.pem())).unwrap();
        fs::write(&pem.key, key.serialize_pem()).unwrap();
        let identity = load(&Tls::DefaultPem {
            pem,
            trusted: None,
            state_dir: temp.0.join("unused"),
            sans: vec![],
        })
        .unwrap()
        .unwrap();
        for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
            let served = handshake(
                identity.config.clone(),
                &[issuer.der().clone()],
                "127.0.0.1",
                true,
                version,
            );
            assert_eq!(served, [leaf.der().clone(), issuer.der().clone()]);
        }
    }
}

#[test]
fn rsa_default_keys_are_validated_and_served_with_tls12_and_tls13() {
    let temp = Temp::new();
    let key = rcgen::KeyPair::from_pem(RSA_KEY).unwrap();
    let cert = rcgen::CertificateParams::new(names(&["127.0.0.1"]))
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let pem = DefaultPem {
        cert: temp.0.join("rsa.pem"),
        key: temp.0.join("rsa-key.pem"),
    };
    fs::write(&pem.cert, cert.pem()).unwrap();
    fs::write(&pem.key, RSA_KEY).unwrap();
    let identity = load(&Tls::DefaultPem {
        pem,
        trusted: None,
        state_dir: temp.0.join("unused"),
        sans: vec![],
    })
    .unwrap()
    .unwrap();
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        assert_eq!(
            handshake(
                identity.config.clone(),
                &[cert.der().clone()],
                "127.0.0.1",
                true,
                version
            ),
            [cert.der().clone()]
        );
    }
}

#[test]
fn unsupported_and_malformed_keys_are_rejected_before_replacing_any_identity() {
    let temp = Temp::new();
    let (pem, _) = imported(&temp);
    let original = load(&Tls::DefaultPem {
        pem: pem.clone(),
        trusted: None,
        state_dir: temp.0.join("unused"),
        sans: vec![],
    })
    .unwrap()
    .unwrap();
    for key in [P521_KEY, "invalid private key"] {
        fs::write(&pem.key, key).unwrap();
        assert!(replacement(
            &temp.0.join("unused"),
            None,
            &[],
            DefaultUpdate::Replace(pem.clone()),
            original.default_key.clone()
        )
        .is_err());
        assert!(!temp.0.join("unused").exists());
        assert_eq!(
            handshake(
                original.config.clone(),
                &original.default_key.as_ref().unwrap().cert,
                "127.0.0.1",
                true,
                &rustls::version::TLS13
            )[0],
            original.default_key.as_ref().unwrap().cert[0]
        );
    }
}

// Test-only RSA key generated locally; ring cannot generate RSA keys at runtime.
const RSA_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCXFOIbqmKplSlD
Tie24O0RDGPj2fq24uNyCF1UwjTmXjFn3fDnp/4SyKhvXBZoZmktyQsiEqoEbRm4
ntmNgbfIeJomPet5clSmRHg5ZfLdV28wApJyTjIvCM34G+myvBOAkC4UihDC/QxL
UXl+YF1oTzkLQS3a5ov25wyJ3bX5KainO/SDfxCM9/MVwk4NfFQTdP4y9vHy8vIM
RnKL7Ql4ARq4Zxs0zQ489EHh12fl93Dyt1PscApuISUe2KBXT0Zou7uZGLH4JRF0
GEbzvFo88od8+/4+gHPW6+zPGkMcExnCy8F+b382wz9oFyW4hnYcfuTTOWkt6ZoG
kaixrvMZAgMBAAECggEADnYIQn+nm/XUEkYr1PxolKYr9YZK0jCbdrHIRTGloJtJ
fVioE3RoBGJChQ46ctnCnYhN0YAZ1/ydZqeFyV6uObkZgCmTHXfNFHEfVNcmTPtu
gBju7ZQAh7H/DQ2W6+3GrBqg3oRsoE1aXOc6itcseayDmEkv23EP2p04/QVEwsmN
XLv+41BEUc4KuFvdCMoTcCx18VIMVRweERNefSkClSgZfz6W/lR1QKh9o/IdOwP3
NLrIsiQmMNNau7dZwg380JF9KGSf1cbTYJhPS+QMQm6SRAdvFiL3MmOM70nRrEc0
N5/cna8nFx6nhPxiEwAVR13k/SZpSMAFG2jKRSbqtQKBgQDH5xUHxwCNdLYGU7V+
6ZZCz9h4Ju/GeQHV2tZMvkC8zRcIlf4XHd8NQlJPy06n1ZIUKJtRdURYKZVgxoCN
Ag/xPYIOa0VPEuya+MCRXFJffTqApXhLGHkNG9D8ooXPl7z55T4w55XbDxb+iMhd
03K69DdhR4UPJbDimiaf65XwVwKBgQDBeoQRc9itC1jMN1q9ZvHaKSFF8ZVn/ahW
ycNq33IdfBPfzUcC/bDY0UMM9sDbR2ZKw5SLYrDDfZNqmfZ62dlQP72A3UaqLH0E
XEIgGM7Qfbx8wfqn3JwjtOmV/bMPaA9r8SNzD70AfxkRLigCUZ/xNkUh4tEvvx5m
ZgPhUCZSDwKBgHpCWFhjW3EoWlNQSwaNsjq2h7wkhC0swPScXMie61UNUfMMWVSm
Dk87qr/mpfp74lW16BhP0jbhZ9zCSsjy4/E1Hx/Wd5M/DbpSPOQqRK7mFiusCABV
vWNJCe4ZMCGguhFUJU8ftEU/215dJb5MnjS9PEmgxj5XXPGaOdlA6nkNAoGBALZa
Cm+IiAdQUnjnExbAF+JOMZYCg5ZndGmX8B6iSqOPdgvRJcSj5jHzyQZG6q3BNZ/x
hed5NLCLgo1OXwiamj8QVENISnnJRGr/qTMnLoVCfIv5J41eV6svr5g2bLVwllhe
BdNEWhZFtBKFNiA5Xk2xoKa3aAFgV/6a171cFUHxAoGAannT5g5EYSVmNfhHMMRo
qD06tq3ui/Paiilx+GG1EHy368zh0ztXIifKcVAcz2nRt1ryhnmYyNdN6JvoHuOt
C7KFaKHD6+A+W3pvpPfGIgqUXTzn4E+KWVAIPWCViI48u8lBXiya26CGVLmpgeFt
LCkcsfooCGTle97RW6c+Zwg=
-----END PRIVATE KEY-----
"#;

// A valid SEC1 key on a curve unsupported by the ring TLS signer.
const P521_KEY: &str = r#"-----BEGIN EC PRIVATE KEY-----
MIHcAgEBBEIBZXWjqeX8MuyywXXAscam+COnOHTr8OJg29EThJ+Skkfd3HxaDm8r
AO3atT65PRtYzhcLRyP7WhK5X1yJc7S6bHSgBwYFK4EEACOhgYkDgYYABAD7eD5X
ZSaUCZv3FIBZ4vYta45tuTTwcMpXnqBX7/TfbBobIUM751zpdApp+kx+A6mUywsE
b00cw8IDaaeuPSYXUAAoagEgQqROgvOPMEnsCSeu/DsyloBAd8/H/B2Ij9jlXOHt
rl9zNYRW+qdR5NqfKBbylQ5rF07LeYlyjCwiuTr0UQ==
-----END EC PRIVATE KEY-----
"#;
