//! The `Poh` module provides an object for generating a Proof of History.
//! It records Hashes items on behalf of its users.
use hash::{hash, hashv, Hash};

pub struct Poh {
    prev_id: Hash,
    id: Hash,
    num_hashes: u64,
    pub tick_height: u64,
}

#[derive(Debug)]
pub struct PohEntry {
    pub prev_id: Hash,
    pub num_hashes: u64,
    pub id: Hash,
    pub mixin: Option<Hash>,
}

impl Poh {
    pub fn new(prev_id: Hash, tick_height: u64) -> Self {
        Poh {
            prev_id,
            num_hashes: 0,
            id: prev_id,
            tick_height,
        }
    }

    pub fn hash(&mut self) {
        self.id = hash(&self.id.as_ref());
        self.num_hashes += 1;
    }

    pub fn record(&mut self, mixin: Hash) -> PohEntry {
        self.id = hashv(&[&self.id.as_ref(), &mixin.as_ref()]);

        let prev_id = self.prev_id;
        self.prev_id = self.id;

        let num_hashes = self.num_hashes + 1;
        self.num_hashes = 0;

        PohEntry {
            prev_id,
            num_hashes,
            id: self.id,
            mixin: Some(mixin),
        }
    }

    // emissions of Ticks (i.e. PohEntries without a mixin) allows
    //  validators to parallelize the work of catching up
    pub fn tick(&mut self) -> PohEntry {
        self.hash();

        let num_hashes = self.num_hashes;
        self.num_hashes = 0;

        let prev_id = self.prev_id;
        self.prev_id = self.id;

        self.tick_height += 1;

        PohEntry {
            prev_id,
            num_hashes,
            id: self.id,
            mixin: None,
        }
    }
}

#[cfg(test)]
pub fn verify(initial: Hash, entries: &[PohEntry]) -> bool {
    let mut prev_id = initial;

    for entry in entries {
        assert!(entry.num_hashes != 0);
        assert!(prev_id == entry.prev_id);

        for _ in 1..entry.num_hashes {
            prev_id = hash(&prev_id.as_ref());
        }
        prev_id = match entry.mixin {
            Some(mixin) => hashv(&[&prev_id.as_ref(), &mixin.as_ref()]),
            None => hash(&prev_id.as_ref()),
        };
        if prev_id != entry.id {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use hash::Hash;
    use poh::{self, PohEntry};

    #[test]
    #[should_panic]
    fn test_poh_verify_assert() {
        poh::verify(
            Hash::default(),
            &[PohEntry {
                prev_id: Hash::default(),
                num_hashes: 0,
                id: Hash::default(),
                mixin: None,
            }],
        );
    }

}
