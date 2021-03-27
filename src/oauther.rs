use crate::{FilterConfig, util};
use oauth2::{ClientSecret, ClientId, TokenUrl, PkceCodeChallenge, AuthUrl, RedirectUrl, CsrfToken, Scope, PkceCodeVerifier};
use url;
use cookie::{Cookie, CookieBuilder};
use crate::oauther::Response::{NewAction, NewState};
use url::{Url, ParseError};
use oauth2::basic::BasicClient;
use getrandom;
use std::cell::RefCell;
use std::ops::DerefMut;
use oauth2::http::{HeaderMap, HeaderValue};
use std::time;
use std::time::Duration;
use oauth2::http::header::SET_COOKIE;

pub struct OAuther {
    config: OAutherConfig,
    state: Box<dyn State>,
    client: BasicClient,
    cache: Box<dyn Cache>,
}

pub enum Action {
    Noop,
    Redirect(Url, HeaderMap),
    HttpCall,
    Allow
}

trait State {
    fn handle_request(&self, oauther: &OAuther, header: &Vec<(&str, &str)>) -> Response;
}

pub trait Cache {
    fn get_tokens_for_session(&self, session: &String) -> Option<Vec<String>>;
    fn set_tokens_for_session(&mut self, session: String, tokens: Vec<String>);

    fn get_verfier_for_state(&self, state: &String) -> Option<&PkceCodeVerifier>;
    fn set_verfier_for_state(&mut self, state: &String, verifier: &PkceCodeVerifier);
}

impl OAuther {
    pub fn new(
        config: FilterConfig,
        cache: Box<dyn Cache>,
    ) -> Result<OAuther, ParseError> {
        let auther_config = OAutherConfig::from(config);

        let client = BasicClient::new(
                auther_config.client_id.clone(),
                Some(auther_config.client_secret.clone()),
                AuthUrl::from_url(auther_config.authorization_url.clone()),
                Some(TokenUrl::from_url(auther_config.token_url.clone()))
            )
                .set_redirect_url(RedirectUrl::from_url(auther_config.redirect_url.clone()));

        Ok(OAuther {
            config: auther_config,
            state: Box::new(Start { }),
            client,
            cache,
        })
    }

    fn handle_request_header(&mut self, headers: Vec<(&str, &str)>) -> Action {

        match self.state.handle_request(self, &headers) {
            Response::NewState(state) => {
                self.state = state;
                self.handle_request_header(headers)
            }
            Response::NewAction(action) => match action {
                OAutherAction::Redirect(url, headers, update) => {
                    update(self);
                    Action::Redirect(url, headers)
                }
                _ => Action::Noop,
            }
        }
    }

    pub fn session_cookie(&self, headers: &Vec<(&str, &str)>) -> Option<String> {
        let cookies: Option<&(&str, &str)> =
            headers.iter().find( |(name, value)| { *name == "cookie" } );
        return match cookies {
            Some(cookies) => {
                let cookies: Vec<&str> = cookies.1.split(";").collect();
                for cookie_string in cookies {
                    let cookie_name_end = cookie_string.find('=').unwrap_or(0);
                    let cookie_name = &cookie_string[0..cookie_name_end];
                    if cookie_name.trim() == self.config.cookie_name {
                        return Some(cookie_string[(cookie_name_end + 1)..cookie_string.len()].to_string().to_owned());
                    }
                }
                None
            },
            None => None
        }
    }

    fn create_session_cookie(&self) -> Cookie {
        CookieBuilder::new(
            self.config.cookie_name.as_str().to_owned(),
            util::new_random_verifier(32).secret().to_owned())
            .secure(true)
            .http_only(true)
            .finish()
    }

    fn authorization_server_redirect(&self) -> (Url, Box<dyn Fn(&mut OAuther) -> ()>) {
        // TODO cache verifier for use in the token call

        let verifier = util::new_random_verifier(32);
        let pkce_challenge=
            PkceCodeChallenge::from_code_verifier_sha256(&verifier);

        let (auth_url, csrf_token) = self.client
            .authorize_url(|| CsrfToken::new("state123".to_string()))
            // Set the desired scopes.
            .add_scope(Scope::new("openid".to_string()))
            // Set the PKCE code challenge.
            .set_pkce_challenge(pkce_challenge)
            .url();

        let closure_state = csrf_token.secret().clone();
        let closure_verifier = PkceCodeVerifier::new(verifier.secret().clone());
        (
            auth_url,
            Box::new(
                move |  oauther|
                    { oauther.cache.set_verfier_for_state(&closure_state, &closure_verifier)}))
    }
}

impl State for Start  {

    fn handle_request(&self, oauther: &OAuther, headers: &Vec<(&str, &str)>) -> Response {
        // check cookie
        match oauther.session_cookie(headers) {
            Some(cookie) => NewState(Box::new(CookieFound { })),
            None => {
                let (url, update) = oauther.authorization_server_redirect();
                let mut headers = HeaderMap::new();
                headers.insert(SET_COOKIE, oauther.create_session_cookie().to_string().parse().unwrap());
                NewAction(OAutherAction::Redirect(url, headers,  update))
            },
        }
    }
}

impl State for CookieFound {

    fn handle_request(&self, _oauther: &OAuther, _header: &Vec<(&str, &str)>) -> Response {
        NewAction(OAutherAction::Noop)
    }
}


enum Response {
    NewState(Box<dyn State>),
    NewAction(OAutherAction),
}

enum OAutherAction {
    Noop,
    Redirect(Url, HeaderMap, Box<dyn Fn(&mut OAuther) -> ()>),
    HttpCall,
    Allow
}

struct Start { }
struct CookieFound {  }

struct OAutherConfig {
    cookie_name: String,
    auth_cluster: String,
    redirect_url: url::Url,
    authorization_url: url::Url,
    token_url: url::Url,
    client_id: ClientId,
    client_secret: ClientSecret
}

impl OAutherConfig {
    fn from(config: FilterConfig) -> OAutherConfig {
        OAutherConfig {
            cookie_name: config.cookie_name,
            auth_cluster: config.auth_cluster,
            redirect_url: url::Url::parse(config.redirect_uri.as_str())
                .expect("Error parsing FilterConfig redirect_uri when creating OAutherConfig"),
            authorization_url: url::Url::parse(config.auth_uri.as_str())
                .expect("Error parsing FilterConfig auth_uri when creating OAutherConfig"),
            token_url: url::Url::parse(config.token_uri.as_str())
                .expect("Error parsing FilterConfig token_uri when creating OAutherConfig"),
            client_id: ClientId::new(config.client_id),
            client_secret: ClientSecret::new(config.client_secret),
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;
    use crate::oauther::OAutherAction::Redirect;
    use std::borrow::Borrow;
    use std::collections::HashMap;

    struct TestCache {
        sessions: HashMap<String, Vec<String>>,
        verifiers: HashMap<String, PkceCodeVerifier>
    }
    impl Cache for TestCache {
        fn get_tokens_for_session(&self, session: &String) -> Option<Vec<String>> {
            if let Some(tokens) = self.sessions.get(session) {
                return Some(tokens.to_owned())
            };
            None
        }

        fn set_tokens_for_session(&mut self, session: String, tokens: Vec<String>) {
            self.sessions.insert(session, tokens);
        }

        fn get_verfier_for_state(&self, state: &String) -> Option<&PkceCodeVerifier> {
            if let Some(verifier) = self.verifiers.get(state) {
                return Some(verifier)
            };
            None
        }

        fn set_verfier_for_state(&mut self, state: &String, verifier: &PkceCodeVerifier) {
            self.verifiers.insert(state.to_string(), PkceCodeVerifier::new(verifier.secret().to_string()));
        }
    }

    fn test_config() -> FilterConfig {
        FilterConfig {
            redirect_uri: "http://redirect".to_string(),
            target_header_name: "".to_string(),
            cookie_name: "sessioncookie".to_string(),
            auth_cluster: "some_cluster".to_string(),
            issuer: "".to_string(),
            auth_uri: "http://authorization".to_string(),
            token_uri: "http://token".to_string(),
            client_id: "myclient".to_string(),
            client_secret: "mysecret".to_string()
        }
    }

    fn test_oauther() -> OAuther {
        OAuther::new(
            test_config(),
            Box::new(TestCache { sessions: HashMap::new(), verifiers: HashMap::new() }),
        ).unwrap()
    }

    #[test]
    fn new() {
        let oauther= test_oauther();
        assert_eq!(
            oauther.config.authorization_url.as_str(),
            "http://authorization/"
        );
    }

    #[test]
    fn unauthorized_request() {
        let mut oauther = test_oauther();

        let action = oauther.handle_request_header(vec![("random_header", "value")]);

        if let Action::Redirect(url, headers) = action {
            assert_eq!(url.origin().unicode_serialization().as_str(), "http://authorization");
            let result = oauther.cache.get_verfier_for_state(&"state123".to_string());
            assert_ne!(result.unwrap().secret(), "");
            assert!(headers.contains_key("set-cookie"));
        } else { panic!("action was not redirect, action" ) }

    }

    #[test]
    fn session_cookie_present_request() {
        let mut oauther= test_oauther();
        let action = oauther.handle_request_header(vec![("sessioncookie", "value")]);
        assert_eq!(action.type_id(), Action::Allow.type_id());
    }

    #[test]
    fn code_grant_redirect() {
        let mut oauther = test_oauther();
        let action = oauther.handle_request_header(vec![(":path", "auth/?code=awesomecode&state=state123")]);
    }
}