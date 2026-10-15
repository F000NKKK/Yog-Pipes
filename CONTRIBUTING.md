# Contributing to Yog-Pipes

Thanks for wanting to help with Yog-Pipes, an abstract pipe network for Yog mods. Bug reports, ideas, documentation
fixes and code are all welcome.

## Reporting a bug

Open an [issue](https://github.com/F000NKKK/Yog-Pipes/issues/new/choose) with:

- what you did, what you expected and what happened instead;
- your operating system, Minecraft version and loader (Fabric, NeoForge or Forge);
- the log or a screenshot, if there is one.

Search the existing issues first — someone may have reported it already.
Security problems are not reported in issues: see [SECURITY.md](SECURITY.md).

## Suggesting a feature

Open an issue describing the problem you want solved before writing a lot of
code, so we can agree on the approach first.

## Making a change

1. Fork the repository and create a branch from `master`.
2. Make the change, with tests where it makes sense.
3. Check that everything builds and the tests pass:

   ```bash
   yog check
   cargo test --manifest-path .yog-build/Cargo.toml
   yog build
   ```

4. Open a pull request that explains what changed and why.

The core stays one abstract pipe; typed pipes (fluids, energy, signals) belong in add-on mods.

## Style

- Code reads like the code around it: same naming, idioms and comment density.
- Rust is formatted with `cargo fmt`; TypeScript follows the existing formatting.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):
  `feat(scope): …`, `fix(scope): …`, `docs: …`, `build(deps): …`, `chore: …`.
- The project is in active development: an outdated API is replaced, not kept
  alongside the new one.

## License

By contributing you agree that your contribution is licensed under the
project's [license](LICENSE).

## Code of conduct

Everyone taking part is expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
