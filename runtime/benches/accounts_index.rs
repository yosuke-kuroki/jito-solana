#![feature(test)]

extern crate test;

use rand::{thread_rng, Rng};
use solana_runtime::{accounts_db::AccountInfo, accounts_index::AccountsIndex};
use solana_sdk::pubkey::{self, Pubkey};
use std::collections::HashSet;
use test::Bencher;

#[bench]
fn bench_accounts_index(bencher: &mut Bencher) {
    const NUM_PUBKEYS: usize = 10_000;
    let pubkeys: Vec<_> = (0..NUM_PUBKEYS).map(|_| pubkey::new_rand()).collect();

    const NUM_FORKS: u64 = 16;

    let mut reclaims = vec![];
    let index = AccountsIndex::<AccountInfo>::default();
    for f in 0..NUM_FORKS {
        for pubkey in pubkeys.iter().take(NUM_PUBKEYS) {
            index.upsert(
                f,
                pubkey,
                &Pubkey::default(),
                &[],
                &HashSet::new(),
                AccountInfo::default(),
                &mut reclaims,
            );
        }
    }

    let mut fork = NUM_FORKS;
    let mut root = 0;
    bencher.iter(|| {
        for _p in 0..NUM_PUBKEYS {
            let pubkey = thread_rng().gen_range(0, NUM_PUBKEYS);
            index.upsert(
                fork,
                &pubkeys[pubkey],
                &Pubkey::default(),
                &[],
                &HashSet::new(),
                AccountInfo::default(),
                &mut reclaims,
            );
            reclaims.clear();
        }
        index.add_root(root);
        root += 1;
        fork += 1;
    });
}
