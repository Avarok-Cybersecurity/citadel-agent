//! Each redirect hop is decided, resolved and pinned again: same host, at most three.

use super::*;
use citadel_internal_service_types::AppFetchRefusal;

fn only_acme() -> AppNetGrant {
    grant(false, &["api.acme.com"])
}

#[tokio::test]
async fn a_same_host_redirect_is_followed_and_pinned_again() {
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    w.resolver.answer("api.acme.com", &["93.184.216.35"]);
    w.connector.reply(redirect(302, "/v2/items"));
    let fetched = w
        .fetch(&only_acme(), &get("https://api.acme.com/v1/items"))
        .await
        .expect("followed");
    assert_eq!(fetched.redirects, 1);
    let attempts = w.connector.attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[1].url, "https://api.acme.com:443/v2/items");
    assert_eq!(
        attempts[1].connect,
        vec!["93.184.216.35:443".parse().unwrap()],
        "the second hop was not resolved and pinned again"
    );
}

#[tokio::test]
async fn a_redirect_to_a_private_address_is_refused_before_the_second_hop() {
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    w.resolver.answer("api.acme.com", &["192.168.0.10"]);
    w.connector
        .reply(redirect(307, "https://api.acme.com/next"));
    let result = w.fetch(&only_acme(), &get("https://api.acme.com/")).await;
    assert_eq!(result.map(|_| ()), Err(AppFetchRefusal::Policy));
    assert_eq!(w.connector.attempts().len(), 1);
}

#[tokio::test]
async fn an_off_host_or_downgrading_redirect_is_refused() {
    for location in [
        "https://evil.example/",
        "https://169.254.169.254/latest/meta-data",
        "http://api.acme.com/",
        "//evil.example/",
    ] {
        let w = world();
        w.resolver.answer("api.acme.com", &["93.184.216.34"]);
        w.resolver.answer("evil.example", &["93.184.216.99"]);
        w.connector.reply(redirect(301, location));
        let both = grant(false, &["api.acme.com", "evil.example"]);
        let result = w.fetch(&both, &get("https://api.acme.com/")).await;
        assert_eq!(
            result.map(|_| ()),
            Err(AppFetchRefusal::Policy),
            "{location}"
        );
        assert_eq!(w.connector.attempts().len(), 1, "{location}");
    }
}

#[tokio::test]
async fn three_redirects_are_followed_and_a_fourth_is_refused() {
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    for n in 1..=4 {
        w.connector.reply(redirect(302, &format!("/hop{n}")));
    }
    let result = w.fetch(&only_acme(), &get("https://api.acme.com/")).await;
    assert_eq!(result.map(|_| ()), Err(AppFetchRefusal::Policy));
    assert_eq!(
        w.connector.attempts().len(),
        4,
        "the original and three hops"
    );

    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    for n in 1..=3 {
        w.connector.reply(redirect(308, &format!("/hop{n}")));
    }
    let fetched = w
        .fetch(&only_acme(), &get("https://api.acme.com/"))
        .await
        .expect("three are allowed");
    assert_eq!(fetched.redirects, 3);
}

#[tokio::test]
async fn a_see_other_turns_a_post_into_a_bodiless_get() {
    let w = world();
    w.resolver.answer("api.acme.com", &["93.184.216.34"]);
    w.connector.reply(redirect(303, "/done"));
    let mut post = get("https://api.acme.com/submit");
    post.method = "POST".into();
    post.body = b"{\"a\":1}".to_vec();
    w.fetch(&only_acme(), &post).await.expect("followed");
    let attempts = w.connector.attempts();
    assert_eq!(attempts[0].method, "POST");
    assert_eq!(attempts[0].body, post.body);
    assert_eq!(attempts[1].method, "GET");
    assert!(attempts[1].body.is_empty());
    assert_eq!(w.net.meter.read(&key()).bytes_out, 7);
}
