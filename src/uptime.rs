//! Does this domain actually answer, and how fast?
//!
//! A domain in the panel is a routing rule, not a promise. It can point at a
//! service that was renamed, a port nothing listens on, or a container that
//! stopped — and the panel will keep showing the rule as if it were fine. The
//! only way to know a domain works is to ask it.
//!
//! **Only the domains the operator enrols are watched**, never all of them. On a
//! host with 713 domains most are aliases, redirects and parked names; a list
//! that watches everything is a list nobody reads. Enrolling is a deliberate act
//! and the watchlist is therefore itself information: this is what matters. It
//! also means the checks stay cheap enough to run in front of you and wait.
//!
//! Latency is the first thing worth knowing, but a single number is close to
//! meaningless: "500 ms" is only slow relative to something. Two comparisons
//! make it mean something, and neither needs any stored history:
//!
//! - **Split the time.** The wait until the first byte of the RESPONSE HEAD is
//!   the server thinking; the rest is the body coming down the wire. A high
//!   time-to-first-byte with a fast finish is a slow application; a fast start
//!   with a slow finish is a big payload or a slow link.
//! - **Compare a domain with its peers.** Checking the whole watchlist at once
//!   gives a median to judge against, so the answer is "these three are eight
//!   times the median" rather than a number nobody can calibrate.
//!
//! This module holds what a check IS, what its answer MEANS, and which results
//! deserve attention. Every one of those judgements is pure and tested without a
//! network; `send` at the bottom is the single place that actually goes out.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The HTTP methods a check can use. A check is often more than a GET: an API
/// worth monitoring may only answer properly to a POST with a body.
pub const METHODS: &[&str] = &["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"];

/// What to send to one URL, and what counts as a good answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    /// Extra request headers, in order. Kept as pairs rather than a map because
    /// a request may legitimately repeat a header name.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The status this URL is EXPECTED to answer with. `None` means "any 2xx or
    /// 3xx is fine", which is the right default for a web page but wrong for an
    /// endpoint that correctly answers 401 to an unauthenticated probe — calling
    /// that "down" would be a false alarm the operator learns to ignore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<u16>,
    /// How long to wait before calling it unreachable.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_method() -> String {
    "GET".into()
}

fn default_timeout_secs() -> u64 {
    10
}

impl Check {
    /// The check a domain gets when nobody has configured one: fetch it and see.
    pub fn get(url: &str) -> Self {
        Self {
            url: url.to_string(),
            method: default_method(),
            headers: Vec::new(),
            body: None,
            expect: None,
            timeout_secs: default_timeout_secs(),
        }
    }

    /// Why this check cannot be sent, in the user's words.
    pub fn problem(&self) -> Option<String> {
        if !self.url.starts_with("http://") && !self.url.starts_with("https://") {
            return Some(format!("'{}' is not an http(s) URL", self.url));
        }
        if !METHODS.contains(&self.method.as_str()) {
            return Some(format!("'{}' is not an HTTP method", self.method));
        }
        // A GET with a body is legal but almost always a mistake in a form: the
        // user picked a method and typed a payload, then changed the method back.
        if matches!(self.method.as_str(), "GET" | "HEAD") && self.body.is_some() {
            return Some(format!("a {} cannot carry a body", self.method));
        }
        if let Some((name, _)) = self.headers.iter().find(|(n, _)| n.trim().is_empty()) {
            return Some(format!("a header has no name (value '{name}')"));
        }
        None
    }
}

/// What came back.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The server answered. `head` is the wait until the response head arrived —
    /// the server thinking — and `total` includes reading the body.
    Answered {
        status: u16,
        head: Duration,
        total: Duration,
    },
    /// Nothing usable came back: DNS, connection refused, TLS, timeout.
    Failed(String),
}

/// One check and its answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub url: String,
    pub outcome: Outcome,
}

/// What the answer MEANS. Deliberately three states and not two: a domain that
/// answers with the wrong status is a different problem from one that does not
/// answer at all, and they get fixed in different places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Answered as expected.
    Working,
    /// Answered, but not with a status that means "this is serving".
    Unexpected,
    /// No usable answer at all.
    Unreachable,
}

impl Probe {
    pub fn verdict(&self, check: &Check) -> Verdict {
        match &self.outcome {
            Outcome::Failed(_) => Verdict::Unreachable,
            Outcome::Answered { status, .. } => match check.expect {
                Some(want) if want == *status => Verdict::Working,
                Some(_) => Verdict::Unexpected,
                // A redirect is a working domain: it is how a canonical host, an
                // http→https jump and a login wall all answer. Calling those
                // "down" is the false alarm that teaches people to ignore alarms.
                None if (200..400).contains(status) => Verdict::Working,
                None => Verdict::Unexpected,
            },
        }
    }

    /// The server-thinking time, when there was one.
    pub fn head(&self) -> Option<Duration> {
        match self.outcome {
            Outcome::Answered { head, .. } => Some(head),
            Outcome::Failed(_) => None,
        }
    }
}

/// The middle time-to-first-byte of everything that answered.
///
/// The median rather than the mean: one 10-second timeout would drag a mean far
/// enough to hide every real outlier behind it.
pub fn median_head(probes: &[Probe]) -> Option<Duration> {
    let mut times: Vec<Duration> = probes.iter().filter_map(Probe::head).collect();
    if times.is_empty() {
        return None;
    }
    times.sort();
    Some(times[times.len() / 2])
}

/// How many times the median this probe took. `None` when it did not answer, or
/// when there is nothing to compare against.
///
/// This is what makes a latency figure mean anything without keeping history: a
/// domain is judged against its own peers, measured through the same network,
/// from the same machine, at the same moment.
pub fn slowness(probe: &Probe, median: Option<Duration>) -> Option<f64> {
    let (head, median) = (probe.head()?, median?);
    if median.is_zero() {
        return None;
    }
    Some(head.as_secs_f64() / median.as_secs_f64())
}

/// A duration as the user reads it. Milliseconds up to a second, then seconds —
/// nobody wants to count the digits in "12480 ms".
pub fn human(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

/// The watchlist in the order it should be READ: what is broken first, then the
/// slowest, then the rest — each paired with its last answer, if there is one.
///
/// Not the order it was enrolled in. A monitor is looked at when something feels
/// wrong, and the answer must be at the top: burying one unreachable domain
/// among twenty healthy ones is how a list stops being read at all.
pub fn ranked<'a>(watch: &'a [Check], probes: &'a [Probe]) -> Vec<(&'a Check, Option<&'a Probe>)> {
    let mut rows: Vec<(&Check, Option<&Probe>)> = watch
        .iter()
        .map(|c| (c, probes.iter().find(|p| p.url == c.url)))
        .collect();
    rows.sort_by(|(ca, pa), (cb, pb)| {
        let rank = |c: &Check, p: &Option<&Probe>| match p {
            // Not yet checked sorts last: it is not a finding, it is an absence.
            None => 3,
            Some(p) => match p.verdict(c) {
                Verdict::Unreachable => 0,
                Verdict::Unexpected => 1,
                Verdict::Working => 2,
            },
        };
        rank(ca, pa)
            .cmp(&rank(cb, pb))
            .then(pb.and_then(|p| p.head()).cmp(&pa.and_then(|p| p.head())))
            .then(ca.url.cmp(&cb.url))
    });
    rows
}

/// Headers as the user edits them: one `Name: value` per line.
///
/// A form field per header would cap how many you can have and make reordering
/// impossible; this is the shape people already paste from curl and browser
/// devtools.
pub fn headers_from_text(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| match l.split_once(':') {
            Some((name, value)) => (name.trim().to_string(), value.trim().to_string()),
            // No colon: keep it as a name so `problem()` can complain about it
            // by name, rather than silently dropping what the user typed.
            None => (l.trim().to_string(), String::new()),
        })
        .collect()
}

/// The inverse, for filling the form back in.
pub fn headers_to_text(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .map(|(n, v)| format!("{n}: {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------- The one place that actually sends something ----------

/// Send one check and time it.
///
/// Redirects are deliberately NOT followed. The question is whether THIS domain
/// and path answer, and following the hop would report on a different URL — and
/// time it too, so a slow destination would be blamed on a redirect that was
/// itself instant.
pub fn send(http: &reqwest::blocking::Client, check: &Check) -> Probe {
    let probe = |outcome| Probe {
        url: check.url.clone(),
        outcome,
    };
    if let Some(problem) = check.problem() {
        return probe(Outcome::Failed(problem));
    }
    let method = match reqwest::Method::from_bytes(check.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => return probe(Outcome::Failed(format!("bad method '{}'", check.method))),
    };
    let mut req = http
        .request(method, &check.url)
        .timeout(Duration::from_secs(check.timeout_secs));
    for (name, value) in &check.headers {
        req = req.header(name, value);
    }
    if let Some(body) = &check.body {
        req = req.body(body.clone());
    }

    let started = Instant::now();
    match req.send() {
        Err(e) => probe(Outcome::Failed(reason(&e))),
        Ok(resp) => {
            // `send` returns once the response HEAD is in — that wait is the
            // server thinking. Reading the body is the wire.
            let head = started.elapsed();
            let status = resp.status().as_u16();
            let read = resp.bytes();
            let total = started.elapsed();
            match read {
                Ok(_) => probe(Outcome::Answered {
                    status,
                    head,
                    total,
                }),
                // Headers arrived and then the body died: that is a real failure,
                // and reporting the status alone would call it working.
                Err(e) => probe(Outcome::Failed(format!(
                    "answered {status}, then {}",
                    reason(&e)
                ))),
            }
        }
    }
}

/// A reqwest error in the words an operator needs.
///
/// Its own Display is a chain like "error sending request for url (…): operation
/// timed out", which buries the one word that says what to fix.
fn reason(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return "timed out".into();
    }
    if e.is_connect() {
        return "could not connect".into();
    }
    if e.is_redirect() {
        return "too many redirects".into();
    }
    // The innermost cause is the useful one (DNS, TLS, refused).
    let mut src: &dyn std::error::Error = e;
    while let Some(next) = src.source() {
        src = next;
    }
    src.to_string()
}

/// Send every check, a few at a time, and keep the order of the list.
///
/// Concurrent because a watchlist is checked in front of a waiting user: twenty
/// domains at one second each is twenty seconds of staring. Bounded because the
/// point is to measure latency, and a hundred simultaneous requests would
/// measure the local machine's contention instead.
pub fn send_all(checks: &[Check]) -> Vec<Probe> {
    const AT_ONCE: usize = 8;
    let http = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("easypanel-cli/", env!("CARGO_PKG_VERSION")))
        .build();
    let http = match http {
        Ok(c) => c,
        Err(e) => {
            return checks
                .iter()
                .map(|c| Probe {
                    url: c.url.clone(),
                    outcome: Outcome::Failed(format!("no HTTP client: {e}")),
                })
                .collect()
        }
    };
    let mut out = Vec::with_capacity(checks.len());
    for group in checks.chunks(AT_ONCE) {
        let handles: Vec<_> = group
            .iter()
            .map(|check| {
                let (http, check) = (http.clone(), check.clone());
                std::thread::spawn(move || send(&http, &check))
            })
            .collect();
        for h in handles {
            match h.join() {
                Ok(p) => out.push(p),
                // A panicked prober must not take the whole run down, and must
                // not silently vanish from a list the user is counting.
                Err(_) => out.push(Probe {
                    url: "?".into(),
                    outcome: Outcome::Failed("the check crashed".into()),
                }),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answered(url: &str, status: u16, head_ms: u64, total_ms: u64) -> Probe {
        Probe {
            url: url.into(),
            outcome: Outcome::Answered {
                status,
                head: Duration::from_millis(head_ms),
                total: Duration::from_millis(total_ms),
            },
        }
    }

    #[test]
    fn the_broken_ones_are_read_first_then_the_slowest() {
        let watch: Vec<Check> = ["fine", "slow", "dead", "wrong", "unchecked"]
            .iter()
            .map(|n| Check::get(&format!("https://{n}.test/")))
            .collect();
        let probes = vec![
            answered("https://fine.test/", 200, 50, 55),
            answered("https://slow.test/", 200, 900, 950),
            Probe {
                url: "https://dead.test/".into(),
                outcome: Outcome::Failed("could not connect".into()),
            },
            answered("https://wrong.test/", 502, 20, 25),
        ];
        let order: Vec<&str> = ranked(&watch, &probes)
            .iter()
            .map(|(c, _)| c.url.as_str())
            .collect();
        assert_eq!(
            order,
            vec![
                "https://dead.test/",  // unreachable
                "https://wrong.test/", // answered, but wrongly
                "https://slow.test/",  // working, slowest first
                "https://fine.test/",
                "https://unchecked.test/", // an absence, not a finding
            ]
        );
    }

    #[test]
    fn headers_round_trip_through_the_text_the_user_edits() {
        let text = "Authorization: Bearer abc\nX-Trace: 1";
        let parsed = headers_from_text(text);
        assert_eq!(
            parsed,
            vec![
                ("Authorization".to_string(), "Bearer abc".to_string()),
                ("X-Trace".to_string(), "1".to_string())
            ]
        );
        assert_eq!(headers_to_text(&parsed), text);
        // A value containing a colon (a URL) must not be cut at the first one.
        assert_eq!(
            headers_from_text("Referer: https://a.test/x")[0].1,
            "https://a.test/x"
        );
        // Blank lines are not headers; a line with no colon is kept so the check
        // can complain about it instead of dropping what was typed.
        assert_eq!(headers_from_text("\n\nOops\n").len(), 1);
        assert!(Check {
            headers: headers_from_text("  : value"),
            ..Check::get("https://a.test/")
        }
        .problem()
        .is_some());
    }

    #[test]
    fn a_redirect_is_reported_as_itself_not_followed_to_somewhere_else() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let redirect = server.mock(|when, then| {
            when.method(GET).path("/old");
            then.status(301).header("location", "/new");
        });
        let destination = server.mock(|when, then| {
            when.method(GET).path("/new");
            then.status(200).body("hello");
        });

        let http = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let check = Check::get(&server.url("/old"));
        let probe = send(&http, &check);

        // The question is whether THIS path answers. Following the hop would
        // report on a different URL and time it too, so a slow destination would
        // be blamed on a redirect that was itself instant.
        match probe.outcome {
            Outcome::Answered { status, .. } => assert_eq!(status, 301),
            other => panic!("expected the redirect itself, got {other:?}"),
        }
        assert_eq!(probe.verdict(&check), Verdict::Working);
        redirect.assert();
        destination.assert_hits(0);
    }

    #[test]
    fn a_post_check_sends_its_body_and_headers() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        // Exactly what the owner asked for: a method with a payload, because an
        // API worth watching may only answer properly to a real request.
        let api = server.mock(|when, then| {
            when.method(POST)
                .path("/api/health")
                .header("authorization", "Bearer secret")
                .body(r#"{"ping":true}"#);
            then.status(401);
        });

        let http = reqwest::blocking::Client::new();
        let check = Check {
            method: "POST".into(),
            headers: vec![("Authorization".into(), "Bearer secret".into())],
            body: Some(r#"{"ping":true}"#.into()),
            expect: Some(401),
            ..Check::get(&server.url("/api/health"))
        };
        let probe = send(&http, &check);
        api.assert();
        // An authenticated endpoint answering 401 is behaving correctly.
        assert_eq!(probe.verdict(&check), Verdict::Working);
        assert!(probe.head().is_some());
    }

    #[test]
    fn a_dead_address_is_unreachable_and_says_why_in_two_words() {
        let http = reqwest::blocking::Client::new();
        // Port 1 on localhost: nothing listens there.
        let check = Check {
            timeout_secs: 2,
            ..Check::get("http://127.0.0.1:1/")
        };
        let probe = send(&http, &check);
        assert_eq!(probe.verdict(&check), Verdict::Unreachable);
        match probe.outcome {
            Outcome::Failed(why) => assert!(
                why.len() < 60 && !why.contains("error sending request"),
                "the reason must be readable, got: {why}"
            ),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn a_check_that_cannot_be_sent_fails_without_touching_the_network() {
        let http = reqwest::blocking::Client::new();
        let probe = send(&http, &Check::get("not-a-url"));
        match probe.outcome {
            Outcome::Failed(why) => assert!(why.contains("not an http(s) URL"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_redirect_is_a_working_domain_not_a_failure() {
        let check = Check::get("https://a.test/");
        // http→https, a canonical host and a login wall all answer like this.
        // Calling them down is the false alarm that teaches people to ignore
        // alarms — the one failure mode a monitor cannot afford.
        assert_eq!(answered("u", 301, 10, 10).verdict(&check), Verdict::Working);
        assert_eq!(answered("u", 200, 10, 10).verdict(&check), Verdict::Working);
        assert_eq!(
            answered("u", 502, 10, 10).verdict(&check),
            Verdict::Unexpected
        );
    }

    #[test]
    fn an_endpoint_that_should_answer_401_is_working_when_it_does() {
        // An authenticated API answering 401 to an unauthenticated probe is
        // behaving correctly; a 200 from it would be the alarming answer.
        let check = Check {
            expect: Some(401),
            ..Check::get("https://api.test/")
        };
        assert_eq!(answered("u", 401, 5, 5).verdict(&check), Verdict::Working);
        assert_eq!(
            answered("u", 200, 5, 5).verdict(&check),
            Verdict::Unexpected
        );
    }

    #[test]
    fn not_answering_is_a_different_problem_from_answering_badly() {
        let check = Check::get("https://a.test/");
        let dead = Probe {
            url: "u".into(),
            outcome: Outcome::Failed("connection refused".into()),
        };
        assert_eq!(dead.verdict(&check), Verdict::Unreachable);
        assert_ne!(
            dead.verdict(&check),
            answered("u", 502, 1, 1).verdict(&check)
        );
    }

    #[test]
    fn one_timeout_does_not_drag_the_yardstick_past_everything_slow() {
        // The median, not the mean: a single 10 s timeout would pull a mean up
        // past everything that is genuinely slow, and nothing would be flagged
        // on exactly the run where it matters.
        let mut probes: Vec<Probe> = (0..10).map(|_| answered("fast", 200, 90, 95)).collect();
        probes.push(answered("slow", 200, 400, 410));
        probes.push(answered("stuck", 200, 10_000, 10_000));

        let median = median_head(&probes);
        assert_eq!(median, Some(Duration::from_millis(90)));
        // A mean would sit above 900 ms and call the 400 ms one faster than
        // average — the opposite of the truth.
        assert!(slowness(&probes[10], median).unwrap() > 4.0);
        assert!(slowness(&probes[11], median).unwrap() > 100.0);
        assert!(slowness(&probes[0], median).unwrap() < 1.5);
    }

    #[test]
    fn a_probe_that_never_answered_has_no_slowness_to_report() {
        // It is unreachable, which is a louder finding than "slow" — it must not
        // be quietly filed among the slow ones, nor counted in the median.
        let dead = Probe {
            url: "u".into(),
            outcome: Outcome::Failed("timed out".into()),
        };
        assert_eq!(slowness(&dead, Some(Duration::from_millis(50))), None);
        assert_eq!(median_head(&[dead]), None);
    }

    #[test]
    fn a_check_refuses_what_cannot_be_sent() {
        assert!(Check::get("https://a.test/").problem().is_none());
        assert!(Check::get("a.test").problem().is_some(), "not a URL");
        let bad_method = Check {
            method: "FETCH".into(),
            ..Check::get("https://a.test/")
        };
        assert!(bad_method.problem().is_some());
        // Picking POST, typing a body, then switching back to GET is the easy
        // mistake a form makes; say so rather than silently dropping the body.
        let get_with_body = Check {
            body: Some("{}".into()),
            ..Check::get("https://a.test/")
        };
        assert!(get_with_body
            .problem()
            .unwrap()
            .contains("cannot carry a body"));
    }
}
