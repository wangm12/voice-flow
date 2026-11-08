# Staged license normalizer regression tests

These standard-library `unittest` cases exercise the normalizer beside this directory.
They use temporary fixtures and do not run packaging, Swift, Cargo, the app, or models.

Run from any working directory inside the repository:

```sh
python3 -m unittest discover -s "$(git rev-parse --show-toplevel)/src-tauri/mlx-sidecar/scripts/tests" -p 'test_*.py' -v
```
