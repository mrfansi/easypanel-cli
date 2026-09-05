//! AWS Signature v4 **query-string presigning** for S3-compatible object storage
//! (used against Cloudflare R2 for `db dump`).
//!
//! A presigned URL carries the auth in the query string, so a plain `curl -T file
//! '<url>'` inside a container can upload to R2 without the secret ever leaving
//! this process — only the short-lived signature and the *access key id* (not the
//! secret) travel to the container. We sign for R2 the same way as S3: `service=s3`,
//! `region` from the provider ("auto" for R2), payload hash the literal
//! `UNSIGNED-PAYLOAD`, path-style `{endpoint}/{bucket}/{key}`, and only `host` signed.
//!
//! The signing primitives are tested against AWS's own documented Sig v4 example
//! (see the tests), so this isn't reverse-engineered guesswork.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const SERVICE: &str = "s3";
const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";

/// Lowercase hex, the encoding AWS uses for every hash and the final signature.
fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex_lower(&h.finalize())
}

fn hmac(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

/// RFC 3986 encoding as AWS defines it for canonical requests: only
/// `A-Z a-z 0-9 - _ . ~` pass through; everything else is `%XX` uppercase.
/// `keep_slash` leaves `/` unescaped — true for a path, false for a query value
/// (where the credential's `/`s must become `%2F`).
fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if unreserved || (keep_slash && b == b'/') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(
                char::from_digit((b >> 4) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
            out.push(
                char::from_digit((b & 0xf) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
        }
    }
    out
}

/// The signing key: HMAC chained over date → region → service → "aws4_request",
/// seeded with `"AWS4" + secret`. This is the per-day, per-region key AWS derives.
fn signing_key(secret: &str, datestamp: &str, region: &str) -> Vec<u8> {
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), datestamp.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, SERVICE.as_bytes());
    hmac(&k_service, b"aws4_request")
}

/// The final signature (lowercase hex) for a fully-formed canonical request.
/// Split out so the AWS documented vector can be replayed exactly in tests.
fn sign(
    secret: &str,
    amz_date: &str,
    datestamp: &str,
    region: &str,
    canonical_request: &str,
) -> String {
    let scope = format!("{datestamp}/{region}/{SERVICE}/aws4_request");
    let string_to_sign = format!(
        "{ALGORITHM}\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    hex_lower(&hmac(
        &signing_key(secret, datestamp, region),
        string_to_sign.as_bytes(),
    ))
}

/// Build a presigned URL for `method` (`GET`/`PUT`/`DELETE`) against a path-style
/// S3/R2 object. `amz_date` is `YYYYMMDDTHHMMSSZ`; `expires` is seconds until the
/// URL stops working. `endpoint` is the account endpoint (scheme optional).
// A presigner genuinely needs all of these; a params struct would be ceremony.
#[allow(clippy::too_many_arguments)]
pub(crate) fn presign(
    method: &str,
    endpoint: &str,
    bucket: &str,
    key: &str,
    access_key: &str,
    secret_key: &str,
    region: &str,
    amz_date: &str,
    expires: u32,
) -> String {
    let host = endpoint
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let datestamp = &amz_date[..8];

    // Path-style URI: /{bucket}/{key}, each key segment encoded but "/" kept.
    let enc_key = key
        .split('/')
        .map(|seg| uri_encode(seg, false))
        .collect::<Vec<_>>()
        .join("/");
    let canonical_uri = format!("/{bucket}/{enc_key}");

    // Query params MUST be sorted by name; these five already are.
    let credential = format!("{access_key}/{datestamp}/{region}/{SERVICE}/aws4_request");
    let pairs = [
        ("X-Amz-Algorithm", ALGORITHM.to_string()),
        ("X-Amz-Credential", credential),
        ("X-Amz-Date", amz_date.to_string()),
        ("X-Amz-Expires", expires.to_string()),
        ("X-Amz-SignedHeaders", "host".to_string()),
    ];
    let canonical_query = pairs
        .iter()
        .map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false)))
        .collect::<Vec<_>>()
        .join("&");

    let canonical_headers = format!("host:{host}\n");
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\nhost\n{UNSIGNED_PAYLOAD}"
    );

    let sig = sign(secret_key, amz_date, datestamp, region, &canonical_request);
    format!("https://{host}{canonical_uri}?{canonical_query}&X-Amz-Signature={sig}")
}

/// Sign a `GET` ListObjectsV2 for objects under `prefix` and return
/// `(url, authorization_header)`. Unlike [`presign`] this uses HEADER auth (the
/// signature travels in `Authorization`), because listing is a one-off request the
/// tool makes itself — the caller sends the URL with three headers: this
/// `Authorization`, `x-amz-date: {amz_date}`, and `x-amz-content-sha256:
/// UNSIGNED-PAYLOAD`. Used to find the dumps this tool wrote, since EasyPanel has no
/// endpoint that lists them.
///
/// `continuation_token` asks for the NEXT page: a listing returns at most 1000
/// keys, and a caller that signs only the first page silently reports part of a
/// bucket as all of it.
// Same shape as `presign`: a signer genuinely needs all of these, and a params
// struct would be ceremony.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sign_list(
    endpoint: &str,
    bucket: &str,
    prefix: &str,
    continuation_token: Option<&str>,
    access_key: &str,
    secret_key: &str,
    region: &str,
    amz_date: &str,
) -> (String, String) {
    let host = endpoint
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let datestamp = &amz_date[..8];

    // The canonical query MUST be sorted by parameter name in byte order, and a
    // continuation token sorts BEFORE both of the others
    // (`continuation-token` < `list-type` < `prefix`) — hence a sort rather than
    // a hand-written string, which is how a second parameter breaks a signature.
    let mut params = vec![
        ("list-type", uri_encode("2", false)),
        ("prefix", uri_encode(prefix, false)),
    ];
    if let Some(token) = continuation_token {
        // The token is opaque base64 (`/`, `+`, `=`), so it must be encoded as a
        // query VALUE — slashes included.
        params.push(("continuation-token", uri_encode(token, false)));
    }
    params.sort_by_key(|(name, _)| *name);
    let canonical_query = params
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let canonical_uri = format!("/{bucket}/");
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_headers =
        format!("host:{host}\nx-amz-content-sha256:{UNSIGNED_PAYLOAD}\nx-amz-date:{amz_date}\n");
    let canonical_request = format!(
        "GET\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{UNSIGNED_PAYLOAD}"
    );
    let sig = sign(secret_key, amz_date, datestamp, region, &canonical_request);
    let credential = format!("{access_key}/{datestamp}/{region}/{SERVICE}/aws4_request");
    let auth = format!(
        "{ALGORITHM} Credential={credential}, SignedHeaders={signed_headers}, Signature={sig}"
    );
    (
        format!("https://{host}{canonical_uri}?{canonical_query}"),
        auth,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // AWS's own documented Sig v4 example — "Example: GET Object (using query
    // parameters)". Fixed inputs with a published canonical-request hash and
    // signature; reproducing them proves the primitives, not just their shape.
    // https://docs.aws.amazon.com/AmazonS3/latest/API/sigv4-query-string-auth.html
    const EX_SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    fn example_canonical_request() -> String {
        // Exactly as AWS documents it (virtual-hosted host, /test.txt).
        [
            "GET",
            "/test.txt",
            "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host",
            "host:examplebucket.s3.amazonaws.com",
            "",
            "host",
            "UNSIGNED-PAYLOAD",
        ]
        .join("\n")
    }

    #[test]
    fn canonical_request_hash_matches_aws_documented_value() {
        assert_eq!(
            sha256_hex(example_canonical_request().as_bytes()),
            "3bfa292879f6447bbcda7001decf97f4a54dc650c8942174ae0a9121cf58ad04"
        );
    }

    #[test]
    fn signature_matches_aws_documented_value() {
        let sig = sign(
            EX_SECRET,
            "20130524T000000Z",
            "20130524",
            "us-east-1",
            &example_canonical_request(),
        );
        assert_eq!(
            sig,
            "aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );
    }

    #[test]
    fn uri_encode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(uri_encode("aA9-_.~", false), "aA9-_.~");
        assert_eq!(uri_encode("a/b", true), "a/b");
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
        assert_eq!(uri_encode("a b+c", false), "a%20b%2Bc");
    }

    #[test]
    fn presign_is_path_style_and_carries_every_required_param() {
        let url = presign(
            "PUT",
            "https://acct.r2.cloudflarestorage.com",
            "mybucket",
            "zzz-r2dump/db-20260101.sql.gz",
            "AKIAEXAMPLE",
            "secretexample",
            "auto",
            "20260101T000000Z",
            900,
        );
        // Path-style: host then /bucket/key (the key's "/" is preserved).
        assert!(url.starts_with(
            "https://acct.r2.cloudflarestorage.com/mybucket/zzz-r2dump/db-20260101.sql.gz?"
        ));
        for needle in [
            "X-Amz-Algorithm=AWS4-HMAC-SHA256",
            "X-Amz-Credential=AKIAEXAMPLE%2F20260101%2Fauto%2Fs3%2Faws4_request",
            "X-Amz-Date=20260101T000000Z",
            "X-Amz-Expires=900",
            "X-Amz-SignedHeaders=host",
            "&X-Amz-Signature=",
        ] {
            assert!(url.contains(needle), "missing {needle} in {url}");
        }
        // The signature is 64 lowercase hex chars.
        let sig = url.rsplit("X-Amz-Signature=").next().unwrap();
        assert_eq!(sig.len(), 64);
        assert!(sig
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    }
}
