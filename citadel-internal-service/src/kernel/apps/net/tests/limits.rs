//! The policy's caps: requests per minute, response size and time.

use super::*;
use citadel_app_net_policy::{MAX_REQUESTS_PER_MINUTE, MAX_RESPONSE_BODY_BYTES};
use citadel_internal_service_types::{AppFetchLimit, AppFetchRefusal};

fn only_acme() -> AppNetGrant {
    grant(false, &["api.acme.com"])
}

#[tokio::test]
async fn the_sixty_first_request_in_a_minute_never_reaches_the_connector() {
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    for _ in 0..MAX_REQUESTS_PER_MINUTE {
        w.fetch(&only_acme(), &get("https://api.acme.com/"))
            .await
            .expect("within the limit");
    }
    let result = w.fetch(&only_acme(), &get("https://api.acme.com/")).await;
    assert_eq!(
        result.map(|_| ()),
        Err(AppFetchRefusal::Limit(AppFetchLimit::Rate))
    );
    assert_eq!(
        w.connector.attempts().len(),
        MAX_REQUESTS_PER_MINUTE as usize
    );
    w.clock.0.store(60_000, std::sync::atomic::Ordering::SeqCst);
    assert!(w
        .fetch(&only_acme(), &get("https://api.acme.com/"))
        .await
        .is_ok());
}

#[tokio::test]
async fn a_policy_refusal_spends_from_the_same_budget() {
    let w = world();
    for _ in 0..MAX_REQUESTS_PER_MINUTE {
        let _ = w.fetch(&only_acme(), &get("https://evil.example/")).await;
    }
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    let result = w.fetch(&only_acme(), &get("https://api.acme.com/")).await;
    assert_eq!(
        result.map(|_| ()),
        Err(AppFetchRefusal::Limit(AppFetchLimit::Rate))
    );
    assert_eq!(
        w.net.meter.read(&key()).refused,
        u64::from(MAX_REQUESTS_PER_MINUTE) + 1
    );
}

#[tokio::test]
async fn a_body_past_the_cap_is_refused_and_stops_being_read() {
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    let half = vec![0u8; (MAX_RESPONSE_BODY_BYTES / 2) as usize];
    w.connector.reply(Reply {
        status: 200,
        headers: vec![],
        chunks: vec![half.clone(), half.clone(), vec![1], vec![2; 1024]],
    });
    let result = w.fetch(&only_acme(), &get("https://api.acme.com/")).await;
    assert_eq!(
        result.map(|_| ()),
        Err(AppFetchRefusal::Limit(AppFetchLimit::ResponseBody))
    );
    // Exactly the cap, plus the one chunk that crossed it: nothing after was read.
    assert_eq!(
        w.net.meter.read(&key()).bytes_in,
        MAX_RESPONSE_BODY_BYTES + 1
    );

    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    w.connector.reply(Reply {
        status: 200,
        headers: vec![],
        chunks: vec![half.clone(), half],
    });
    let whole = w
        .fetch(&only_acme(), &get("https://api.acme.com/"))
        .await
        .expect("exactly the cap is allowed");
    assert_eq!(whole.body.len() as u64, MAX_RESPONSE_BODY_BYTES);
}

#[tokio::test(start_paused = true)]
async fn an_upstream_that_never_answers_is_abandoned_at_the_time_budget() {
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    w.connector
        .hangs
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let started = Instant::now();
    let result = w.fetch(&only_acme(), &get("https://api.acme.com/")).await;
    assert_eq!(
        result.map(|_| ()),
        Err(AppFetchRefusal::Limit(AppFetchLimit::Time))
    );
    assert_eq!(
        started.elapsed(),
        Duration::from_millis(citadel_app_net_policy::REQUEST_TIMEOUT_MS)
    );
}

#[tokio::test]
async fn response_headers_shown_to_the_app_are_bounded() {
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    let mut headers: Vec<(String, String)> = (0..40)
        .map(|n| (format!("x-h{n}"), "v".to_string()))
        .collect();
    headers.insert(0, ("x-huge".into(), "a".repeat(2048)));
    w.connector.reply(Reply {
        status: 200,
        headers,
        chunks: vec![],
    });
    let fetched = w
        .fetch(&only_acme(), &get("https://api.acme.com/"))
        .await
        .expect("allowed");
    assert_eq!(fetched.headers.len(), citadel_app_net_policy::MAX_HEADERS);
    assert!(fetched.headers.iter().all(|h| h.name != "x-huge"));
}

#[tokio::test]
async fn a_seventeenth_app_in_a_minute_is_refused_before_anything_connects() {
    use crate::kernel::apps::net::rate::MAX_APPS_PER_ACCOUNT;
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    for app in 0..MAX_APPS_PER_ACCOUNT {
        let app_id = format!("com.acme.app{app}");
        w.fetch_as(&app_id, &only_acme(), &get("https://api.acme.com/"))
            .await
            .expect("within the cap");
    }
    let refused = w
        .fetch_as(
            "com.acme.app16",
            &only_acme(),
            &get("https://api.acme.com/"),
        )
        .await;
    assert_eq!(
        refused.map(|_| ()),
        Err(AppFetchRefusal::Limit(AppFetchLimit::Rate))
    );
    assert_eq!(w.connector.attempts().len(), MAX_APPS_PER_ACCOUNT);
    // The control: an app already within the cap keeps its budget.
    w.fetch_as("com.acme.app0", &only_acme(), &get("https://api.acme.com/"))
        .await
        .expect("a counted app still spends");
}

#[tokio::test]
async fn rotating_the_install_id_does_not_reset_the_budget() {
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    let only = only_acme();
    for n in 0..MAX_REQUESTS_PER_MINUTE {
        let key = InstallKey {
            cid: 42,
            install_id: format!("inst-{n}"),
        };
        let request = get("https://api.acme.com/");
        let input = FetchInput {
            key: &key,
            app_id: APP,
            grant: &only,
            request: &request,
        };
        w.net
            .fetch(input, deadline())
            .await
            .expect("within the limit");
    }
    let fresh = InstallKey {
        cid: 42,
        install_id: "never-seen".into(),
    };
    let request = get("https://api.acme.com/");
    let input = FetchInput {
        key: &fresh,
        app_id: APP,
        grant: &only,
        request: &request,
    };
    assert_eq!(
        w.net.fetch(input, deadline()).await.map(|_| ()),
        Err(AppFetchRefusal::Limit(AppFetchLimit::Rate))
    );
}
