use std::time::{SystemTime, Duration};
use serde::{Serialize, Deserialize};
use crate::cache::SharedCache;
use oauth2::http::{HeaderMap, HeaderValue};
use oauth2::http::header::{AUTHORIZATION, SET_COOKIE};
use proxy_wasm::types::BufferType::UpstreamData;
use cookie::{CookieBuilder, Cookie};
use crate::util;

type Seconds = u32;

pub trait SessionCache {
    fn get(&self, id: &String) -> Option<Session>;
    fn set(&mut self, session: SessionUpdate);
}


#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Session {
    id: String,
    pub data: SessionType,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum SessionType {
    AuthorizationRequest(AuthorizationResponseVerifiers),
    Tokens(AuthorizationTokens),
    Empty
}

impl Session {
    pub fn empty(id: String) -> Session {
        Session { id, data: SessionType::Empty }
    }

    pub fn tokens(id: String, access_token: String, expires_in: Option<Duration>, id_token: Option<String>, refresh_token: Option<String>) -> Session{
        Session {
            id,
            data: SessionType::Tokens(AuthorizationTokens {
                created_at: SystemTime::now(),
                access_token,
                expires_in,
                id_token,
                refresh_token
            }),
        }
    }

    pub fn verifiers(id: String, created_at: SystemTime, request_url: String, state: String, pcke_verifier: Option<String>) -> Session {
        Session::from_verifier(id, AuthorizationResponseVerifiers {
            created_at,
            state: State { path: request_url, csrf_token: state },
            pcke_verifier
        })
    }

    pub fn from_headers(cookie_name: String, headers: Vec<(&str, &str)>, cache: &dyn SessionCache) -> Option<Session> {
        let session = Session::parse_cookie(&cookie_name, &headers);
        match session {
            None => None,
            Some(id) => {
                match cache.get(&id) {
                    None => Some(Session::empty(id)),
                    Some(session) => Some(session)
                }
            }
        }
    }

    pub(crate) fn from_verifier(id: String, verifiers: AuthorizationResponseVerifiers) -> Session {
        Session {id, data: SessionType::AuthorizationRequest(verifiers) }
    }
    pub(crate) fn from_tokens(id: String, tokens: AuthorizationTokens) -> Session{
        Session { id, data: SessionType::Tokens(tokens)}
    }

    fn parse_cookie(id: &String, headers: &Vec<(&str, &str)>) -> Option<String> {
        let cookies: Option<&(&str, &str)> =
            headers.iter().find( |(name, _ )| { *name == "cookie" } );
        return match cookies {
            Some(cookies) => {
                let cookies: Vec<&str> = cookies.1.split(";").collect();
                for cookie_string in cookies {
                    let cookie_name_end = cookie_string.find('=').unwrap_or(0);
                    let cookie_name = &cookie_string[0..cookie_name_end];
                    if cookie_name.trim() == id {
                        return Some(cookie_string[(cookie_name_end + 1)..cookie_string.len()].to_string().to_owned());
                    }
                }
                None
            },
            None => None
        }
    }


    pub fn token_response(&self, access_token: String, expires_in: Option<Duration>, id_token: Option<String>, refresh_token: Option<String>) -> SessionUpdate {
        SessionUpdate { id: self.id.clone(), data: UpdateType::Tokens(AuthorizationTokens {
            created_at: SystemTime::now(),
            access_token,
            expires_in,
            id_token,
            refresh_token
        }) }
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UpdateType {
    AuthorizationRequest(AuthorizationResponseVerifiers),
    Tokens(AuthorizationTokens)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionUpdate {
    pub id: String,
    data: UpdateType,
}

impl SessionUpdate {
    pub fn auth_request(request_url: String, state: String , verifier: String) -> SessionUpdate {
        SessionUpdate {
            id: util::new_random_verifier(32).secret().to_owned(),
            data: UpdateType::AuthorizationRequest(AuthorizationResponseVerifiers {
                created_at: SystemTime::now(),
                state: State { path: request_url, csrf_token: state },
                pcke_verifier: Some(verifier)
            })
        }
    }

    pub fn set_cookie_header(&self, name: &String) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(SET_COOKIE, self.cookie(name).parse().unwrap());
        headers
    }

    pub fn cookie(&self, name: &String) -> String {
        CookieBuilder::new(
            name,
            &self.id)
            .secure(true)
            .http_only(true)
            .finish().to_string()
    }

    pub fn create_session(&self) -> Session {
        match &self.data {
            UpdateType::AuthorizationRequest(verifiers) =>
                Session::from_verifier(self.id.clone(), verifiers.clone()),
            UpdateType::Tokens(tokens) =>
                Session::from_tokens(self.id.clone(), tokens.clone()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationResponseVerifiers {
    created_at: SystemTime,
    state: State,
    pcke_verifier: Option<String>
}

impl AuthorizationResponseVerifiers {

    pub fn request_url(&self) -> String {
        self.state.path.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationTokens {
    created_at: SystemTime,
    access_token: String,
    expires_in: Option<Duration>,
    id_token: Option<String>,
    refresh_token: Option<String>
}

impl AuthorizationTokens {
    pub fn new(
        created_at: SystemTime,
        access_token: String,
        expires_in: Option<Duration>,
        id_token: Option<String>,
        refresh_token: Option<String>) -> AuthorizationTokens {
        AuthorizationTokens {
            created_at,
            access_token,
            expires_in,
            id_token,
            refresh_token,
        }
    }

    pub fn bearer(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(format!("bearer {}",self.access_token).as_str()).unwrap());
        headers
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct State {
    path: String,
    csrf_token: String
}

#[cfg(test)]
mod tests {
    use crate::session::{Session, SessionType, UpdateType, SessionCache, SessionUpdate, AuthorizationResponseVerifiers, State, AuthorizationTokens};
    use std::collections::HashMap;
    use std::time::SystemTime;

    pub struct TestCache {
        sessions: HashMap<String, UpdateType>,
    }

    impl TestCache {
        pub fn new() -> TestCache {
            TestCache {
                sessions: HashMap::new(),
            }
        }
    }

    impl SessionCache for TestCache {
        fn get(&self, id: &String) -> Option<Session> {
            match self.sessions.get(id) {
                None => None,
                Some(session_type ) => match session_type {
                    UpdateType::AuthorizationRequest(verifiers) =>
                        Some(Session::from_verifier(id.clone(), verifiers.clone())),
                    UpdateType::Tokens(tokens) =>
                        Some(Session::from_tokens(id.clone(), tokens.clone())),
                }
            }
        }
        fn set(&mut self, session: SessionUpdate) {
            self.sessions.insert(session.id, session.data);
        }
    }

    #[test]
    fn empty() {
        let session = Session::empty("test".to_string());
        assert!(matches!(session.data, SessionType::Empty));
    }

    #[test]
    fn from_headers() {
        let mut cache = TestCache::new();
        let cookie_name="auth_session".to_string();
        let cookie_value = "testsession".to_string();
        let cookie = format!("{}={}", cookie_name, cookie_value);


        let headers: Vec<(&str, &str)> = vec![("cookie", cookie.as_str())];
        let session: Session = Session::from_headers(cookie_name.clone(), headers.clone(), &cache).unwrap();
        assert!(matches!(session.data, SessionType::Empty));

        cache.set(SessionUpdate { id: cookie_value.clone(), data: UpdateType::AuthorizationRequest(AuthorizationResponseVerifiers {
            created_at: SystemTime::now(),
            state: State { path: "/secure".to_string(), csrf_token: "1234".to_string() },
            pcke_verifier: Some("1234".to_string())
        } ) });

        let session: Session = Session::from_headers(cookie_name.clone(), headers.clone(), &cache).unwrap();
        assert!(matches!(session.data, SessionType::AuthorizationRequest { .. }));

        cache.set(SessionUpdate { id: cookie_value.clone(), data: UpdateType::Tokens(AuthorizationTokens {
            created_at: SystemTime::now(),
            access_token: "SomeJWT".to_string(),
            expires_in: None,
            id_token: None,
            refresh_token: None
        }) });

        let session: Session = Session::from_headers(cookie_name.clone(), headers, &cache).unwrap();
        assert!(matches!(session.data, SessionType::Tokens { .. }));
    }

}