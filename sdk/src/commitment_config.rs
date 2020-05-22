#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CommitmentConfig {
    pub commitment: CommitmentLevel,
}

impl Default for CommitmentConfig {
    fn default() -> Self {
        CommitmentConfig {
            commitment: CommitmentLevel::Max,
        }
    }
}

impl CommitmentConfig {
    pub fn recent() -> Self {
        Self {
            commitment: CommitmentLevel::Recent,
        }
    }

    pub fn max() -> Self {
        Self {
            commitment: CommitmentLevel::Max,
        }
    }

    pub fn root() -> Self {
        Self {
            commitment: CommitmentLevel::Root,
        }
    }

    pub fn single() -> Self {
        Self {
            commitment: CommitmentLevel::Single,
        }
    }

    pub fn single_gossip() -> Self {
        Self {
            commitment: CommitmentLevel::SingleGossip,
        }
    }

    pub fn ok(self) -> Option<Self> {
        if self == Self::default() {
            None
        } else {
            Some(self)
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum CommitmentLevel {
    Max,
    Recent,
    Root,
    Single,
    SingleGossip,
}
