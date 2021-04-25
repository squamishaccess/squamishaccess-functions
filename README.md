# SAS Azure Functions

**[Squamish Access Society](https://squamishaccess.ca/)**
> Advocating for responsible stewardship of the cliffs and crags in the greater Squamish Area.</br>
> Traditional territory of the **Sḵwx̱wú7mesh** [Úxwumixw](https://www.squamish.net/).

----

This repository contains the source code for Azure Functions handling various needs for the Squamish Access Society.

Current functions:
- `Paypal-IPN`: Sign-up payment from PayPal IPNs.
- `Membership-Check`: Membership check by email.

## Repository layout

This is a [Rust](https://www.rust-lang.org/) project. To build, run `cargo build`. If you do not have the rust compiler available, install it with [rustup](https://rustup.rs).

The actual PayPal IPN handler is in `src/ipn_handler.rs`.
Everything else is server setup / azure function compatibility.

The code is formatted using `cargo fmt`. Install via `cargo install rustfmt`.

The following environment variables are accepted (or in `.env`):
- `MAILCHIMP_API_KEY` (required)
- `MAILCHIMP_LIST_ID` (required)
- `PAYPAL_SANDBOX` (optional, for testing)
- `RUST_BACKTRACE` (optional, for backtraces)

### Deploying

Currently only set up to deploy to a windows environment.
Must be built with a `x86_64-pc-windows` Rust toolchain.

- `rm bin/squamishaccess-signup-function-rs.exe`
- `cargo build --release`
- `cp target\release\squamishaccess-functions.exe bin/squamishaccess-functions.exe`
- `cargo clean`
- deploy via Azure Core Tools v3 / VS Code extension

## License

Licensed under the [BlueOak Model License 1.0.0](LICENSE.md) — _[Contributions via DCO 1.1](contributing.md#developers-certificate-of-origin)_

[Tide]: https://github.com/http-rs/tide
