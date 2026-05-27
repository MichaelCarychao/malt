use keyring::Entry;

const SERVICE: &str = "malt";
const USER: &str = "anthropic-api-key";

fn entry() -> keyring::Result<Entry> {
    Entry::new(SERVICE, USER)
}

pub fn set_api_key(key: &str) -> keyring::Result<()> {
    entry()?.set_password(key)
}

pub fn get_api_key() -> keyring::Result<String> {
    entry()?.get_password()
}

pub fn clear_api_key() -> keyring::Result<()> {
    let e = entry()?;
    match e.delete_password() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn has_api_key() -> bool {
    matches!(get_api_key(), Ok(s) if !s.is_empty())
}
