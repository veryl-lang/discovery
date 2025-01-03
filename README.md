# Discovery

Discovery is a tool to gather Veryl projects from GitHub, and check whether these projects can be built continuously.
The gathered information is stored in db/db.json of this repository, and updated by week.

## Usage

To check gathered projects with local `veryl` command.
This can be used to check whether the Veryl compiler built locally breaks the existing projects.

```
$ cargo run -- check
```

To specify version of Veryl compiler, `--veryl-version` option can be used.
This feature requires [verylup](https://github.com/veryl-lang/verylup).

```
$ cargo run -- check --veryl-version 0.13.0
```

## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
