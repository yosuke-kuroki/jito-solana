//! The `weighted_shuffle` module provides an iterator over shuffled weights.

use itertools::Itertools;
use num_traits::{FromPrimitive, ToPrimitive};
use rand::Rng;
use rand_chacha::ChaChaRng;
use std::iter;
use std::ops::Div;

/// Returns a list of indexes shuffled based on the input weights
/// Note - The sum of all weights must not exceed `u64::MAX`
pub fn weighted_shuffle<T>(weights: Vec<T>, rng: ChaChaRng) -> Vec<usize>
where
    T: Copy + PartialOrd + iter::Sum + Div<T, Output = T> + FromPrimitive + ToPrimitive,
{
    let mut rng = rng;
    let total_weight: T = weights.clone().into_iter().sum();
    weights
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            let x = (total_weight / v)
                .to_u64()
                .expect("values > u64::max are not supported");
            (
                i,
                // capture the u64 into u128s to prevent overflow
                (&mut rng).gen_range(1, u128::from(std::u16::MAX)) * u128::from(x),
            )
        })
        .sorted_by(|(_, l_val), (_, r_val)| l_val.cmp(r_val))
        .map(|x| x.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn test_weighted_shuffle_iterator() {
        let mut test_set = [0; 6];
        let mut count = 0;
        let shuffle = weighted_shuffle(vec![50, 10, 2, 1, 1, 1], ChaChaRng::from_seed([0x5a; 32]));
        shuffle.into_iter().for_each(|x| {
            assert_eq!(test_set[x], 0);
            test_set[x] = 1;
            count += 1;
        });
        assert_eq!(count, 6);
    }

    #[test]
    fn test_weighted_shuffle_iterator_large() {
        let mut test_set = [0; 100];
        let mut test_weights = vec![0; 100];
        (0..100).for_each(|i| test_weights[i] = (i + 1) as u64);
        let mut count = 0;
        let shuffle = weighted_shuffle(test_weights, ChaChaRng::from_seed([0xa5; 32]));
        shuffle.into_iter().for_each(|x| {
            assert_eq!(test_set[x], 0);
            test_set[x] = 1;
            count += 1;
        });
        assert_eq!(count, 100);
    }

    #[test]
    fn test_weighted_shuffle_compare() {
        let shuffle = weighted_shuffle(vec![50, 10, 2, 1, 1, 1], ChaChaRng::from_seed([0x5a; 32]));

        let shuffle1 = weighted_shuffle(vec![50, 10, 2, 1, 1, 1], ChaChaRng::from_seed([0x5a; 32]));
        shuffle1
            .into_iter()
            .zip(shuffle.into_iter())
            .for_each(|(x, y)| {
                assert_eq!(x, y);
            });
    }

    #[test]
    fn test_weighted_shuffle_imbalanced() {
        let mut weights = vec![std::u32::MAX as u64; 3];
        weights.push(1);
        let shuffle = weighted_shuffle(weights.clone(), ChaChaRng::from_seed([0x5a; 32]));
        shuffle.into_iter().for_each(|x| {
            if x == weights.len() - 1 {
                assert_eq!(weights[x], 1);
            } else {
                assert_eq!(weights[x], std::u32::MAX as u64);
            }
        });
    }
}
