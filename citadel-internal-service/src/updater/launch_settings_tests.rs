use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|a| a.to_string()).collect()
}

fn settings(json: &str) -> LaunchSettings {
    parse(json.as_bytes()).unwrap()
}

/// What 0.10.6's launcher passed: no identity setting at all.
const OLD: [&str; 6] = [
    "--bind",
    "127.0.0.1:12345",
    "--allowed-origins",
    "https://work.avarok.net",
    "--stun-servers",
    "a:1,b:2,c:3",
];

#[test]
fn a_setting_the_command_lacks_is_appended_and_every_given_flag_is_untouched() {
    let launch = settings(r#"{"v":1,"settings":{"--identity-unlock-idle-secs":"900"}}"#);
    let merged = launch.merged(&args(&OLD));
    assert_eq!(merged[..OLD.len()], args(&OLD)[..]);
    assert_eq!(
        merged[OLD.len()..],
        args(&["--identity-unlock-idle-secs", "900"])[..]
    );
}

#[test]
fn a_flag_the_operator_set_wins_in_either_spelling() {
    let launch = settings(
        r#"{"v":1,"settings":{"--identity-unlock-idle-secs":"900","--stun-servers":"x:1,y:2,z:3"}}"#,
    );
    for given in [
        args(&[
            "--bind",
            "127.0.0.1:1",
            "--identity-unlock-idle-secs",
            "60",
            "--stun-servers",
            "a:1,b:2,c:3",
        ]),
        args(&[
            "--bind",
            "127.0.0.1:1",
            "--identity-unlock-idle-secs=60",
            "--stun-servers=a:1,b:2,c:3",
        ]),
    ] {
        assert_eq!(launch.merged(&given), given);
    }
}

#[test]
fn a_flag_that_only_starts_like_a_setting_does_not_count_as_it() {
    let launch = settings(r#"{"v":1,"settings":{"--stun":"x"}}"#);
    let given = args(&["--stun-servers", "a:1,b:2,c:3"]);
    assert_eq!(
        launch.merged(&given),
        args(&["--stun-servers", "a:1,b:2,c:3", "--stun", "x"])
    );
}

#[test]
fn an_empty_list_adds_nothing() {
    assert_eq!(
        settings(r#"{"v":1,"settings":{}}"#).merged(&args(&OLD)),
        args(&OLD)
    );
}

#[test]
fn an_unknown_format_is_refused_whatever_it_holds() {
    for json in [
        r#"{"v":2,"settings":{"--a":"1"}}"#,
        r#"{"v":2,"flags":[["--a","1"]]}"#,
        r#"{"v":0,"settings":{}}"#,
    ] {
        let why = parse(json.as_bytes()).unwrap_err();
        assert!(why.contains("is not the 1 this agent reads"), "{why}");
    }
}

#[test]
fn malformed_assets_are_refused() {
    for json in [
        "",
        "not json",
        r#"{"settings":{}}"#,
        r#"{"v":1}"#,
        r#"{"v":1,"settings":{},"extra":true}"#,
        r#"{"v":1,"settings":{"--a":1}}"#,
        r#"{"v":1,"settings":{"identity":"1"}}"#,
        r#"{"v":1,"settings":{"-b":"1"}}"#,
        r#"{"v":1,"settings":{"--":"1"}}"#,
        r#"{"v":1,"settings":{"--A":"1"}}"#,
        r#"{"v":1,"settings":{"--a=b":"1"}}"#,
        r#"{"v":1,"settings":{"--a":""}}"#,
        r#"{"v":1,"settings":{"--a":"1\n--dangerous"}}"#,
        r#"{"v":1,"settings":{"--check-launch":"1"}}"#,
    ] {
        let why = parse(json.as_bytes()).expect_err(json);
        assert!(
            why.contains(ASSET_NAME) && why.ends_with("nothing was installed"),
            "{why}"
        );
    }
}
