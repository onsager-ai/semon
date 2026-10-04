#![cfg(feature = "real-ssh-tests")]
use semon_ssh::{Credential, Error, Target, check, discover};
use zeroize::Zeroizing;
#[tokio::test]
async fn real_openssh_authentication_changed_key_and_unreachable() {
    let path = std::env::var("SEMON_SSH_KEY_FILE").expect("synthetic fixture key required");
    let port = std::env::var("SEMON_SSH_PORT")
        .expect("fixture SSH port required")
        .parse()
        .unwrap();
    let target = Target {
        address: "127.0.0.1".parse().unwrap(),
        port,
        username: "fixture".into(),
    };
    let credential = Credential(Zeroizing::new(std::fs::read_to_string(path).unwrap()));
    let pin = discover(&target).await.unwrap();
    assert!(pin.fingerprint().unwrap().starts_with("SHA256:"));
    let result = check(&target, &pin, &credential).await.unwrap();
    assert_eq!(result.platform, "Linux");
    let bad = Target {
        username: "rejected-user".into(),
        ..target.clone()
    };
    assert!(matches!(
        check(&bad, &pin, &credential).await,
        Err(Error::Authentication)
    ));
    let mut changed = pin.clone();
    changed.key.replace_range(40..44, "AAAA");
    assert!(matches!(
        check(&target, &changed, &credential).await,
        Err(Error::HostChanged)
    ));
    let offline = Target { port: 1, ..target };
    assert!(matches!(discover(&offline).await, Err(Error::Unreachable)));
}
