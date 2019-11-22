use std::convert::TryFrom;
use std::error;
use std::fmt;
use std::mem;
use std::str::FromStr;

pub use bs58;

#[repr(transparent)]
#[derive(Serialize, Deserialize, Clone, Copy, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Pubkey([u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsePubkeyError {
    WrongSize,
    Invalid,
}

impl fmt::Display for ParsePubkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ParsePubkeyError: {:?}", self)
    }
}

impl error::Error for ParsePubkeyError {}

impl FromStr for Pubkey {
    type Err = ParsePubkeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let pubkey_vec = bs58::decode(s)
            .into_vec()
            .map_err(|_| ParsePubkeyError::Invalid)?;
        if pubkey_vec.len() != mem::size_of::<Pubkey>() {
            Err(ParsePubkeyError::WrongSize)
        } else {
            Ok(Pubkey::new(&pubkey_vec))
        }
    }
}

impl Pubkey {
    pub fn new(pubkey_vec: &[u8]) -> Self {
        Self(
            <[u8; 32]>::try_from(<&[u8]>::clone(&pubkey_vec))
                .expect("Slice must be the same length as a Pubkey"),
        )
    }

    #[cfg(not(feature = "program"))]
    pub fn new_rand() -> Self {
        Self::new(&rand::random::<[u8; 32]>())
    }

    pub fn log(&self) {
        use crate::log::sol_log_64;
        for (i, k) in self.0.iter().enumerate() {
            sol_log_64(0, 0, 0, i as u64, u64::from(*k));
        }
    }
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl AsRef<[u8]> for Pubkey {
    fn as_ref(&self) -> &[u8] {
        &self.0[..]
    }
}

impl fmt::Debug for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", bs58::encode(self.0).into_string())
    }
}

impl fmt::Display for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", bs58::encode(self.0).into_string())
    }
}

#[cfg(not(feature = "program"))]
pub fn write_pubkey_file(outfile: &str, pubkey: Pubkey) -> Result<(), Box<dyn error::Error>> {
    use std::io::Write;

    let printable = format!("{}", pubkey);
    let serialized = serde_json::to_string(&printable)?;

    if let Some(outdir) = std::path::Path::new(&outfile).parent() {
        std::fs::create_dir_all(outdir)?;
    }
    let mut f = std::fs::File::create(outfile)?;
    f.write_all(&serialized.clone().into_bytes())?;

    Ok(())
}

#[cfg(not(feature = "program"))]
pub fn read_pubkey_file(infile: &str) -> Result<Pubkey, Box<dyn error::Error>> {
    let f = std::fs::File::open(infile.to_string())?;
    let printable: String = serde_json::from_reader(f)?;
    Ok(Pubkey::from_str(&printable)?)
}

/// Convenience macro to declare a static Pubkey and functions to interact with it
///
/// bs58_string: bs58 string representation the program's id
///
/// # Examples
///
/// ```
/// solana_sdk::declare_id!("My!!!11111111111111111111111111111111111111");
/// ```
#[macro_export]
macro_rules!
declare_id(
    ($bs58_string:expr) => (
        use std::str::FromStr;

        $crate::lazy_static::lazy_static! {
            static ref _PUBKEY: $crate::pubkey::Pubkey = {
                match $crate::pubkey::Pubkey::from_str(&$bs58_string) {
                    Ok(pubkey) => pubkey,
                    Err(_) => {
                        let pubkey_vec = $crate::bs58::decode(&$bs58_string)
                            .into_vec()
                            .map_err(|e| panic!("Error: {}, {}", $bs58_string, e))
                            .unwrap();
                        let expected_len = std::mem::size_of::<$crate::pubkey::Pubkey>();
                        let len = pubkey_vec.len();
                        if len != expected_len {
                            panic!(
                                "Error: {}, decoded length {}, expected {}",
                                $bs58_string, len, expected_len);
                        } else {
                            panic!(
                                "Error: {}, not a valid string, cannot determine reason",
                                $bs58_string);
                        }
                    }
                }
            };
        }

        pub fn check_id(id: &$crate::pubkey::Pubkey) -> bool {
            *id == *_PUBKEY
        }

        pub fn id() -> $crate::pubkey::Pubkey {
            *_PUBKEY
        }

        #[cfg(test)]
        #[test]
        fn test_id() {
            assert!(check_id(&id()));
        }
    )
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::remove_file;

    #[test]
    fn pubkey_fromstr() {
        let pubkey = Pubkey::new_rand();
        let mut pubkey_base58_str = bs58::encode(pubkey.0).into_string();

        assert_eq!(pubkey_base58_str.parse::<Pubkey>(), Ok(pubkey));

        pubkey_base58_str.push_str(&bs58::encode(pubkey.0).into_string());
        assert_eq!(
            pubkey_base58_str.parse::<Pubkey>(),
            Err(ParsePubkeyError::WrongSize)
        );

        pubkey_base58_str.truncate(pubkey_base58_str.len() / 2);
        assert_eq!(pubkey_base58_str.parse::<Pubkey>(), Ok(pubkey));

        pubkey_base58_str.truncate(pubkey_base58_str.len() / 2);
        assert_eq!(
            pubkey_base58_str.parse::<Pubkey>(),
            Err(ParsePubkeyError::WrongSize)
        );

        let mut pubkey_base58_str = bs58::encode(pubkey.0).into_string();
        assert_eq!(pubkey_base58_str.parse::<Pubkey>(), Ok(pubkey));

        // throw some non-base58 stuff in there
        pubkey_base58_str.replace_range(..1, "I");
        assert_eq!(
            pubkey_base58_str.parse::<Pubkey>(),
            Err(ParsePubkeyError::Invalid)
        );
    }

    #[test]
    fn test_read_write_pubkey() -> Result<(), Box<dyn error::Error>> {
        let filename = "test_pubkey.json";
        let pubkey = Pubkey::new_rand();
        write_pubkey_file(filename, pubkey)?;
        let read = read_pubkey_file(filename)?;
        assert_eq!(read, pubkey);
        remove_file(filename)?;
        Ok(())
    }
}
