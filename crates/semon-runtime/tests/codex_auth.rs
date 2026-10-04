use semon_runtime::codex_auth::*;
use serde_json::json;

// Sanitized wire shapes from the pinned CLI's generated v2 JSON schemas.
fn start(url: &str) -> LoginResponse {
    serde_json::from_value(json!({
        "type": "chatgptDeviceCode", "loginId": "login-1",
        "verificationUrl": url, "userCode": "ABCD-EFGH"
    }))
    .unwrap()
}
fn completed(id: Option<&str>, success: bool) -> LoginCompleted {
    serde_json::from_value(json!({"loginId": id, "success": success,
        "error": "untrusted sensitive diagnostic"}))
    .unwrap()
}
fn account(kind: &str) -> AccountRead {
    serde_json::from_value(json!({"account": {"type": kind}, "requiresOpenaiAuth": true})).unwrap()
}
fn device() -> LoginFlow {
    LoginFlow::new(Method::ChatGptDeviceCode, CODEX_VERSION).unwrap()
}
const OFFICIAL: &str = "https://auth.openai.com/codex/device";

#[test]
fn public_login_wire_routes_and_secret_diagnostics() {
    let secret = "fixture-api-secret";
    let request = LoginRequest::ApiKey { api_key: secret };
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        json!({"type":"apiKey","apiKey":secret})
    );
    assert!(!format!("{request:?}").contains(secret));
    assert_eq!(
        serde_json::to_value(LoginRequest::DeviceCode).unwrap(),
        json!({"type":"chatgptDeviceCode"})
    );
    for unsupported in ["chatgptAuthTokens", "chatgpt", "amazonBedrock"] {
        assert!(serde_json::from_value::<LoginResponse>(json!({"type":unsupported})).is_err());
    }
}

#[test]
fn device_completion_requires_exact_exchange_and_account_read() {
    let mut flow = device();
    flow.started(start(OFFICIAL)).unwrap();
    assert_eq!(flow.state(), State::AwaitingUser);
    assert_eq!(flow.challenge().unwrap().verification_url(), OFFICIAL);
    assert!(!format!("{flow:?}").contains("ABCD-EFGH"));
    assert!(!format!("{:?}", flow.challenge().unwrap()).contains("login-1"));
    for id in [None, Some("old-login")] {
        assert_eq!(
            flow.completed(completed(id, true)),
            Err(AuthError::UnrelatedCompletion)
        );
        assert_eq!(flow.state(), State::AwaitingUser);
    }
    flow.completed(completed(Some("login-1"), true)).unwrap();
    assert_eq!(flow.state(), State::AwaitingAccountRead);
    assert!(flow.challenge().is_none());
    flow.account_read(account("chatgpt")).unwrap();
    assert_eq!(flow.state(), State::AccountPresent);
    assert_eq!(flow.method(), Method::ChatGptDeviceCode);
    assert_eq!(
        flow.completed(completed(Some("login-1"), true)),
        Err(AuthError::UnexpectedState)
    );
}

#[test]
fn mismatch_never_switches_selected_billing_source() {
    let mut api = LoginFlow::new(Method::OpenAiApiKey, CODEX_VERSION).unwrap();
    api.started(LoginResponse::ApiKey).unwrap();
    assert_eq!(
        api.account_read(account("chatgpt")),
        Err(AuthError::BillingMethodMismatch)
    );
    assert_eq!(api.method(), Method::OpenAiApiKey);
    assert_eq!(api.state(), State::Failed);

    let mut device = device();
    assert_eq!(
        device.started(LoginResponse::ApiKey),
        Err(AuthError::BillingMethodMismatch)
    );
    assert_eq!(device.method(), Method::ChatGptDeviceCode);
    assert_eq!(device.state(), State::Failed);
}

#[test]
fn challenge_rejects_alternate_origins_and_url_interpretations() {
    for url in [
        "http://auth.openai.com/codex/device",
        "https://attacker.invalid/codex/device",
        "https://auth.openai.com.attacker.invalid/codex/device",
        "https://auth.openai.com@attacker.invalid/codex/device",
        "https://auth.openai.com:444/codex/device",
        "https://auth.openai.com/codex/device?redirect=evil",
        "javascript:alert(1)",
    ] {
        let mut flow = device();
        assert_eq!(flow.started(start(url)), Err(AuthError::InvalidChallenge));
        assert_eq!(flow.state(), State::Failed);
        assert!(flow.challenge().is_none());
    }
}

#[test]
fn failure_cancellation_absent_account_and_version_mismatch_are_closed() {
    assert!(matches!(
        LoginFlow::new(Method::OpenAiApiKey, "other-version"),
        Err(AuthError::VersionMismatch)
    ));
    let mut flow = device();
    flow.started(start(OFFICIAL)).unwrap();
    flow.completed(completed(Some("login-1"), false)).unwrap();
    assert_eq!(flow.state(), State::Failed);
    assert!(flow.challenge().is_none());
    assert_eq!(
        flow.account_read(account("chatgpt")),
        Err(AuthError::UnexpectedState)
    );

    let mut flow = device();
    flow.started(start(OFFICIAL)).unwrap();
    flow.cancel();
    assert_eq!(flow.state(), State::Cancelled);
    assert!(flow.challenge().is_none());
    assert_eq!(
        flow.completed(completed(Some("login-1"), true)),
        Err(AuthError::UnexpectedState)
    );

    let mut flow = LoginFlow::new(Method::OpenAiApiKey, CODEX_VERSION).unwrap();
    flow.started(LoginResponse::ApiKey).unwrap();
    let missing =
        serde_json::from_value(json!({"account": null,"requiresOpenaiAuth":true})).unwrap();
    assert_eq!(flow.account_read(missing), Err(AuthError::AccountMissing));
}
