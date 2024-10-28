#![allow(clippy::arithmetic_side_effects)]
use {
    clap::{crate_description, crate_name, value_t, value_t_or_exit, App, Arg},
    std::{fmt::Display, fs, path::PathBuf, str::FromStr},
};

struct Bin {
    slot_min: usize,
    slot_max: usize,
    count: usize,
    min_size: usize,
    max_size: usize,
    sum_size: usize,
    avg: usize,
}

fn pad(width: usize) -> String {
    let mut s = String::new();
    for _i in 0..width {
        s = format!("{s} ");
    }
    s
}

fn get_stars(x: usize, max: usize, width: usize) -> String {
    let mut s = String::new();
    let percent = x * width / max;
    for i in 0..width {
        s = format!("{s}{}", if i <= percent { "*" } else { " " });
    }
    s
}

fn is_parsable<T>(string: String) -> Result<(), String>
where
    T: FromStr,
    T::Err: Display,
{
    string
        .parse::<T>()
        .map(|_| ())
        .map_err(|err| format!("error parsing '{string}': {err}"))
}

fn calc(info: &[(usize, usize)], bin_widths: Vec<usize>, offset: i64) {
    let mut info = info.to_owned();
    info.sort();
    let min = info.first().unwrap().0;
    let max_inclusive = info.last().unwrap().0;
    let outside_slot = 432_000 - offset as usize;
    eprintln!("storages: {}", info.len());
    eprintln!("lowest slot: {min}");
    eprintln!("highest slot: {max_inclusive}");
    eprintln!("slot range: {}", max_inclusive - min + 1);
    eprintln!("ancient boundary: {}", outside_slot);
    eprintln!(
        "number of slots beyond ancient bondary: {}",
        info.iter()
            .filter(|x| x.0 < max_inclusive - outside_slot)
            .count()
    );

    let mut bins = Vec::default();
    for i in 0..bin_widths.len() {
        let next = if i == bin_widths.len() - 1 {
            usize::MAX
        } else {
            bin_widths[i + 1]
        };
        let abin = if bin_widths[i] < outside_slot && outside_slot < next {
            let abin = Bin {
                slot_min: bin_widths[i],
                slot_max: outside_slot,
                count: 0,
                min_size: usize::MAX,
                max_size: 0,
                sum_size: 0,
                avg: 0,
            };
            bins.push(abin);
            Bin {
                slot_min: outside_slot,
                slot_max: next,
                count: 0,
                min_size: usize::MAX,
                max_size: 0,
                sum_size: 0,
                avg: 0,
            }
        } else {
            Bin {
                slot_min: bin_widths[i],
                slot_max: next,
                count: 0,
                min_size: usize::MAX,
                max_size: 0,
                sum_size: 0,
                avg: 0,
            }
        };
        bins.push(abin);
    }
    let mut bin_all = Bin {
        slot_min: 0,
        slot_max: 0,
        count: 0,
        min_size: usize::MAX,
        max_size: 0,
        sum_size: 0,
        avg: 0,
    };
    let mut bin_max = Bin {
        slot_min: 0,
        slot_max: 0,
        count: 0,
        min_size: 0,
        max_size: 0,
        sum_size: 0,
        avg: 0,
    };
    info.into_iter().for_each(|(slot, size)| {
        for bin in bins.iter_mut() {
            let relative = max_inclusive - slot;
            if bin.slot_min <= relative && bin.slot_max > relative {
                // eprintln!("{}, {}, {}, {}", slot, relative, max_inclusive, bin.slot_min);
                bin.count += 1;
                bin.sum_size += size;
                bin.min_size = bin.min_size.min(size);
                bin.max_size = bin.max_size.max(size);

                bin_all.count += 1;
                bin_all.sum_size += size;
                bin_all.min_size = bin_all.min_size.min(size);
                bin_all.max_size = bin_all.max_size.max(size);

                break;
            }
        }
    });
    bins.retain_mut(|bin| {
        if bin.count > 0 {
            bin_max.sum_size = bin_max.sum_size.max(bin.sum_size);
            bin_max.max_size = bin_max.max_size.max(bin.max_size);
            bin_max.count = bin_max.count.max(bin.count);
            bin_max.min_size = bin_max.min_size.max(bin.min_size);
            bin.avg = bin.sum_size / bin.count;
        }
        bin_max.avg = bin_max.avg.max(bin.avg);

        bin.count > 0
    });

    bin_all.avg = bin_all.sum_size / bin_all.count;

    eprintln!("overall stats");
    eprintln!("size {}", bin_all.sum_size);
    eprintln!("count {}", bin_all.count);
    eprintln!("min size {}", bin_all.min_size);
    eprintln!("max size {}", bin_all.max_size);
    eprintln!("avg size {}", bin_all.avg);
    eprintln!("bin width {}", bins[0].slot_max - bins[0].slot_min);

    for i in 0..bins.len() {
        if i > 0 && bins[i - 1].slot_max != bins[i].slot_min {
            eprintln!("...");
        }
        let bin = &bins[i];
        if bin.slot_min == outside_slot {
            eprintln!("{}", String::from_utf8(vec![b'-'; 168]).unwrap());
        }
        let offset = format!("{:8}", bin.slot_min);

        if i == 0 {
            let s = [
                format!("{:8}", "slot age"),
                pad(1),
                format!("{:10}", "count"),
                pad(1),
                format!("{:10}", "min size"),
                pad(1),
                format!("{:10}", "max size"),
                pad(1),
                format!("{:10}", "sum size"),
                pad(1),
                format!("{:10}", "avg size"),
                pad(1),
                format!(",{:>15}", "slot min"),
                format!(",{:>15}", "count"),
                format!(",{:>15}", "sum size"),
                format!(",{:>7}", "% size"),
                format!(",{:>15}", "min size"),
                format!(",{:>15}", "max size"),
                format!(",{:>15}", "avg size"),
            ];
            let mut s2 = String::new();
            s.iter().for_each(|s| {
                s2 = format!("{s2}{s}");
            });
            eprintln!("{s2}");
        }

        let s = [
            offset,
            pad(1),
            get_stars(bin.count, bin_max.count, 10),
            pad(1),
            get_stars(bin.min_size, bin_max.min_size, 10),
            pad(1),
            get_stars(bin.max_size, bin_max.max_size, 10),
            pad(1),
            get_stars(bin.sum_size, bin_max.sum_size, 10),
            pad(1),
            get_stars(bin.avg, bin_max.avg, 10),
            pad(1),
            format!(",{:15}", max_inclusive - bin.slot_min),
            format!(",{:15}", bin.count),
            format!(",{:15}", bin.sum_size),
            format!(",{:6}%", bin.sum_size * 100 / bin_all.sum_size),
            format!(",{:15}", bin.min_size),
            format!(",{:15}", bin.max_size),
            format!(",{:15}", bin.avg),
        ];
        let mut s2 = String::new();
        s.iter().for_each(|s| {
            s2 = format!("{s2}{s}");
        });
        eprintln!("{s2}");
    }
}

fn normal_bin_widths() -> Vec<usize> {
    let mut bin_widths = vec![0];
    let div = 432_000 / 20;
    for i in 1..432_000 {
        let b = i * div;
        if b > 432_000 {
            break;
        }
        bin_widths.push(b);
    }
    bin_widths.push(432_000);
    for i in 1..100000 {
        let b = 432_000 + i * div;
        // if b > max_range {
        // break;
        // }
        bin_widths.push(b);
    }
    bin_widths
}

fn normal_ancient(offset: i64) -> Vec<usize> {
    let mut bin_widths = vec![0];
    bin_widths.push((432_000 - offset) as usize);
    bin_widths
}

fn main() {
    let matches = App::new(crate_name!())
        .about(crate_description!())
        .version(solana_version::version!())
        .arg(
            Arg::with_name("ledger")
                .index(1)
                .takes_value(true)
                .value_name("PATH")
                .help("ledger path"),
        )
        .arg(
            Arg::with_name("offset")
                .long("offset")
                .takes_value(true)
                .value_name("SLOT-OFFSET")
                .validator(is_parsable::<i64>)
                .help("ancient offset"),
        )
        .get_matches();

    let ledger = value_t_or_exit!(matches, "ledger", String);
    let offset = value_t!(matches, "offset", i64).unwrap_or(100_000);
    let path: PathBuf = [&ledger, "accounts", "run"].iter().collect();

    if path.is_dir() {
        let dir = fs::read_dir(&path);
        if let Ok(dir) = dir {
            let mut info = Vec::default();
            for entry in dir.flatten() {
                if let Some(name) = entry.path().file_name() {
                    let name = name.to_str().unwrap().split_once(".").unwrap().0;
                    match fs::metadata(entry.path()) {
                        Ok(meta) => {
                            info.push((name.parse::<usize>().unwrap(), meta.len() as usize));
                        }
                        Err(_) => {
                            // skip when metadata fails. This can happen when you are running this tool while a validator is running.
                            // It could clean something away and delete it after getting the dir but before opening the file.
                            continue;
                        }
                    }
                    // eprintln!("{name}, {len}");
                }
            }
            eprintln!("======== Normal Histogram");
            calc(&info, normal_bin_widths(), offset);
            eprintln!("========");

            eprintln!("\n======== Normal Ancient Histogram");
            calc(&info, normal_ancient(offset), offset);
            eprintln!("========");
        } else {
            panic!("couldn't read folder: {path:?}, {:?}", dir);
        }
    } else {
        panic!("not a folder: {:?}", path);
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_calc() {
        let info = vec![
            (0, 8usize),
            (500, 23usize),
            (501, 100),
            (432_000 - 1, 2),
            (432_000, 1),
            (500_000, 18),
            (1_000_000, 80),
        ];
        let max = info.iter().map(|(slot, _size)| *slot).max().unwrap();
        let base = 1000;
        let info = info
            .into_iter()
            .map(|(slot, size)| (max - slot + base, size))
            .collect::<Vec<_>>();
        let offset = 100_000i64;
        calc(&info, normal_bin_widths(), offset);
        calc(&info, normal_ancient(offset), offset);
    }
}
