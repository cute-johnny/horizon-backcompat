// Session::start's HEAD: the account ViaProxy's online-mode session joins use.
fn backcompat_account(auth: &Option<std::sync::Arc<crate::auth::AuthSession>>) {
    crate::net::backcompat::remember_account(auth.clone());
}
