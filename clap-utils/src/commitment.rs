use crate::ArgConstant;
use clap::Arg;

pub const COMMITMENT_ARG: ArgConstant<'static> = ArgConstant {
    name: "commitment",
    long: "commitment",
    help: "Return information at the selected commitment level",
};

pub fn commitment_arg<'a, 'b>() -> Arg<'a, 'b> {
    commitment_arg_with_default("recent")
}

pub fn commitment_arg_with_default<'a, 'b>(default_value: &'static str) -> Arg<'a, 'b> {
    Arg::with_name(COMMITMENT_ARG.name)
        .long(COMMITMENT_ARG.long)
        .takes_value(true)
        .possible_values(&["recent", "single", "singleGossip", "root", "max"])
        .default_value(default_value)
        .value_name("COMMITMENT_LEVEL")
        .help(COMMITMENT_ARG.help)
}
