use base64::prelude::BASE64_STANDARD;
use base64::read::DecoderReader;
use base64::write::EncoderWriter;

use netrc::Netrc;
use reqwest::header::HeaderValue;
use reqwest::Request;
use std::io::Read;
use std::io::Write;
use url::Url;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Credentials {
    /// The name of the user for authentication.
    ///
    /// Unlike `reqwest`, empty usernames should be encoded as `None` instead of an empty string.
    username: Option<String>,
    /// The password to use for authentication.
    password: Option<String>,
}

impl Credentials {
    pub fn new(username: Option<String>, password: Option<String>) -> Self {
        debug_assert!(
            username.is_none()
                || username
                    .as_ref()
                    .is_some_and(|username| !username.is_empty())
        );
        Self { username, password }
    }

    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.password.is_none() && self.username.is_none()
    }

    /// Return [`Credentials`] for a [`Url`] from a [`Netrc`] file, if any.
    ///
    /// If a username is provided, it must match the login in the netrc file or [`None`] is returned.
    pub fn from_netrc(netrc: &Netrc, url: &Url, username: Option<&str>) -> Option<Self> {
        let host = url.host_str()?;
        let entry = netrc
            .hosts
            .get(host)
            .or_else(|| netrc.hosts.get("default"))?;

        // Ensure the username matches if provided
        if username.is_some_and(|username| username != entry.login) {
            return None;
        };

        Some(Credentials {
            username: Some(entry.login.clone()),
            password: Some(entry.password.clone()),
        })
    }

    /// Parse [`Credentials`] from a URL, if any.
    ///
    /// Returns [`None`] if both [`Url::username`] and [`Url::password`] are not populated.
    pub fn from_url(url: &Url) -> Option<Self> {
        if url.username().is_empty() && url.password().is_none() {
            return None;
        }
        Some(Self {
            // Remove percent-encoding from URL credentials
            // See <https://github.com/pypa/pip/blob/06d21db4ff1ab69665c22a88718a4ea9757ca293/src/pip/_internal/utils/misc.py#L497-L499>
            username: if url.username().is_empty() {
                None
            } else {
                Some(
                    urlencoding::decode(url.username())
                        .expect("An encoded username should always decode")
                        .into_owned(),
                )
            },
            password: url.password().map(|password| {
                urlencoding::decode(password)
                    .expect("An encoded password should always decode")
                    .into_owned()
            }),
        })
    }

    /// Parse [`Credentials`] from an HTTP request, if any.
    ///
    /// Only HTTP Basic Authentication is supported.
    pub fn from_request(request: &Request) -> Option<Self> {
        // First, attempt to retrieve the credentials from the URL
        Self::from_url(request.url()).or(
            // Then, attempt to pull the credentials from the headers
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .map(Self::from_header_value)?,
        )
    }

    /// Parse [`Credentials`] from an authorization header, if any.
    ///
    /// Only HTTP Basic Authentication is supported.
    /// [`None`] will be returned if another authoriziation scheme is detected.
    ///
    /// Panics if the authentication is not conformant to the HTTP Basic Authentication scheme:
    /// - The contents must be base64 encoded
    /// - There must be a `:` separator
    pub(crate) fn from_header_value(header: &HeaderValue) -> Option<Self> {
        let mut value = header.as_bytes().strip_prefix(b"Basic ")?;
        let mut decoder = DecoderReader::new(&mut value, &BASE64_STANDARD);
        let mut buf = String::new();
        decoder
            .read_to_string(&mut buf)
            .expect("HTTP Basic Authentication should be base64 encoded.");
        let (username, password) = buf
            .split_once(':')
            .expect("HTTP Basic Authentication should include a `:` separator.");
        let username = if username.is_empty() {
            None
        } else {
            Some(username.to_string())
        };
        let password = if password.is_empty() {
            None
        } else {
            Some(password.to_string())
        };
        Some(Self::new(username, password))
    }

    /// Create an HTTP Basic Authentication header for the credentials.
    ///
    /// Panics if the username or password cannot be base64 encoded.
    pub(crate) fn to_header_value(&self) -> HeaderValue {
        // See: <https://github.com/seanmonstar/reqwest/blob/2c11ef000b151c2eebeed2c18a7b81042220c6b0/src/util.rs#L3>
        let mut buf = b"Basic ".to_vec();
        {
            let mut encoder = EncoderWriter::new(&mut buf, &BASE64_STANDARD);
            write!(encoder, "{}:", self.username().unwrap_or_default())
                .expect("Write to base64 encoder should succeed");
            if let Some(password) = self.password() {
                write!(encoder, "{}", password).expect("Write to base64 encoder should succeed");
            }
        }
        let mut header = HeaderValue::from_bytes(&buf).expect("base64 is always valid HeaderValue");
        header.set_sensitive(true);
        header
    }

    /// Attach the credentials to the given request.
    ///
    /// Any existing credentials will be overridden.
    #[must_use]
    pub fn authenticate(&self, mut request: reqwest::Request) -> reqwest::Request {
        request
            .headers_mut()
            .insert(reqwest::header::AUTHORIZATION, Self::to_header_value(self));
        request
    }
}

#[cfg(test)]
mod test {
    use insta::assert_debug_snapshot;

    use super::*;

    #[test]
    fn from_url_no_credentials() {
        let url = &Url::parse("https://example.com/simple/first/").unwrap();
        assert_eq!(Credentials::from_url(url), None);
    }

    #[test]
    fn from_url_username_and_password() {
        let url = &Url::parse("https://example.com/simple/first/").unwrap();
        let mut auth_url = url.clone();
        auth_url.set_username("user").unwrap();
        auth_url.set_password(Some("password")).unwrap();
        let credentials = Credentials::from_url(&auth_url).unwrap();
        assert_eq!(credentials.username(), Some("user"));
        assert_eq!(credentials.password(), Some("password"));
    }

    #[test]
    fn from_url_no_username() {
        let url = &Url::parse("https://example.com/simple/first/").unwrap();
        let mut auth_url = url.clone();
        auth_url.set_password(Some("password")).unwrap();
        let credentials = Credentials::from_url(&auth_url).unwrap();
        assert_eq!(credentials.username(), None);
        assert_eq!(credentials.password(), Some("password"));
    }

    #[test]
    fn from_url_no_password() {
        let url = &Url::parse("https://example.com/simple/first/").unwrap();
        let mut auth_url = url.clone();
        auth_url.set_username("user").unwrap();
        let credentials = Credentials::from_url(&auth_url).unwrap();
        assert_eq!(credentials.username(), Some("user"));
        assert_eq!(credentials.password(), None);
    }

    #[test]
    fn authenticated_request_from_url() {
        let url = Url::parse("https://example.com/simple/first/").unwrap();
        let mut auth_url = url.clone();
        auth_url.set_username("user").unwrap();
        auth_url.set_password(Some("password")).unwrap();
        let credentials = Credentials::from_url(&auth_url).unwrap();

        let mut request = reqwest::Request::new(reqwest::Method::GET, url);
        request = credentials.authenticate(request);

        let mut header = request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .expect("Authorization header should be set")
            .clone();
        header.set_sensitive(false);

        assert_debug_snapshot!(header, @r###""Basic dXNlcjpwYXNzd29yZA==""###);
        assert_eq!(Credentials::from_header_value(&header), Some(credentials));
    }

    #[test]
    fn authenticated_request_from_url_with_percent_encoded_user() {
        let url = Url::parse("https://example.com/simple/first/").unwrap();
        let mut auth_url = url.clone();
        auth_url.set_username("user@domain").unwrap();
        auth_url.set_password(Some("password")).unwrap();
        let credentials = Credentials::from_url(&auth_url).unwrap();

        let mut request = reqwest::Request::new(reqwest::Method::GET, url);
        request = credentials.authenticate(request);

        let mut header = request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .expect("Authorization header should be set")
            .clone();
        header.set_sensitive(false);

        assert_debug_snapshot!(header, @r###""Basic dXNlckBkb21haW46cGFzc3dvcmQ=""###);
        assert_eq!(Credentials::from_header_value(&header), Some(credentials));
    }

    #[test]
    fn authenticated_request_from_url_with_percent_encoded_password() {
        let url = Url::parse("https://example.com/simple/first/").unwrap();
        let mut auth_url = url.clone();
        auth_url.set_username("user").unwrap();
        auth_url.set_password(Some("password==")).unwrap();
        let credentials = Credentials::from_url(&auth_url).unwrap();

        let mut request = reqwest::Request::new(reqwest::Method::GET, url);
        request = credentials.authenticate(request);

        let mut header = request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .expect("Authorization header should be set")
            .clone();
        header.set_sensitive(false);

        assert_debug_snapshot!(header, @r###""Basic dXNlcjpwYXNzd29yZD09""###);
        assert_eq!(Credentials::from_header_value(&header), Some(credentials));
    }
}
