Building the Solana Docs
---

Install dependencies, build, and test the docs:

```bash
$ brew install coreutils
$ brew install mscgen
$ cargo install svgbob_cli
$ cargo install mdbook-linkcheck
$ cargo install mdbook
$ ./build.sh
```

Run any Rust tests in the markdown:

```bash
$ make test
```

Render markdown as HTML:

```bash
$ make build
```

Render and view the docs:

```bash
$ make open
```
