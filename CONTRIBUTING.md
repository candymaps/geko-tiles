# Contribuir a geko

## Licencia

geko se publica con la licencia [AGPL-3.0](LICENSE), y cada contribución se acepta con esa misma licencia, sin firmar nada aparte: conservas la autoría de tu código. Solo los colaboradores del repositorio pueden abrir pull requests; si quieres contribuir, pide que te inviten. Si contribuyes en nombre de tu empresa, o parte del código no es tuyo, dilo en el PR.

## Ramas

Todo desarrollo sale de `dev` a una rama propia (`feature/<nombre>`, `bugfix/<nombre>`…) y vuelve a `dev` con un pull request. A `main` solo se llega con un pull request de `dev` a `main`.

## Comprobaciones

Antes de abrir el PR, pasa `make test` (o `ci/test.sh`). Las reglas del proyecto están en [AGENTS.md](AGENTS.md), y la especificación en [PROJECT.md](docs/PROJECT.md).
