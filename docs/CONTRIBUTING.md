# Building and testing

How to build Earbuds, and the Flatpak, is in the [README](../README.md#building).

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

On an immutable host such as Fedora Silverblue there is no Rust and there are
no headers; build in a container:

```sh
toolbox create -d fedora -r 44 earbuds-build
toolbox run -c earbuds-build sudo dnf install -y rust cargo rustfmt clippy gcc \
    pkgconf-pkg-config gtk4-devel libadwaita-devel
toolbox run -c earbuds-build cargo test
```

A toolbox shares the host's system bus and Bluetooth, so `cargo run` in one
talks to the real earbuds.

## Where the tests are

The protocol is where things go wrong quietly, so that is where the tests are:
`src/protocol.rs` and `src/state.rs` are tested on the frames a real Wave Buds 2
sent and received (`tests/fixtures/wave-buds-2.txt`, from jbl-control). Every
request Earbuds makes to read a setting is checked to be byte for byte the one
in that capture.

## Trying it against real earbuds

```sh
EARBUDS_DEBUG=1 cargo run
```

prints how the control link was found, and every frame to (`tx`) and from
(`rx`) the earbuds. With a model other than the Wave Buds 2, that log is what
an issue about it needs.

Two things the earbuds do that look like bugs and are not:

- **Noise cancelling is declined with one earbud in the case.** The earbuds
  answer with the mode they stayed in; Earbuds says so in a toast.
- **The control address changes.** The LE address the settings live on rotates;
  a link that fails is followed by a fresh scan, not a retry of the old address.

## CI

Two jobs. `test` runs formatting, lints and the tests in a Fedora container,
because the runner's Ubuntu does not ship the GTK and libadwaita Earbuds links.
`flatpak` builds the app on pushes to `main`, keeping both the OSTree repository
it exports and the single-file bundle made from it.

The release job waits on CI rather than running alongside it: a build published
from a commit whose tests then failed is worse than no build at all.

## How a build reaches people

A green run on `main` promotes that build into the Flatpak repository served
from GitHub Pages, and replaces the bundle on the rolling `continuous` release.
The repository is the route people install from; the bundle is a fallback. A
bundle is a file, not a remote, so `flatpak update` has nothing to pull from,
and GNOME Software refuses it outright.

The `gh-pages` branch is force-pushed as a fresh orphan commit each time. The
repository keeps its own history inside OSTree, so a git history of it as well
would grow every clone without bound and serve nobody.

### The signing key

Builds are signed, and the public half travels inside `earbuds.flatpakrepo`, so
adding the remote is also what establishes trust. The private half lives in the
`FLATPAK_GPG_PRIVATE_KEY` repository secret, and nothing publishes without it:
the release job stops if it is unset.

To create one, or to roll it:

```sh
export GNUPGHOME=$(mktemp -d) && chmod 700 "$GNUPGHOME"

gpg --batch --gen-key <<KEY
%no-protection
Key-Type: eddsa
Key-Curve: Ed25519
Key-Usage: sign
Name-Real: earbuds repository signing
Name-Email: earbuds@develonrails.github.io
Expire-Date: 0
%commit
KEY

gpg --export-secret-keys --armor | base64 -w0 |
    gh secret set FLATPAK_GPG_PRIVATE_KEY --repo develonrails/earbuds
```

Rolling it re-signs everything on the next run, but a client that already
trusts the old key will refuse the new signatures: `flatpak remote-delete
earbuds` and add it again. Worth avoiding for that reason alone.

