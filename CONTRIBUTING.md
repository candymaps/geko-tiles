# Contribuir a geko

## Licencia y CLA

geko se publica con la licencia [AGPL-3.0](LICENSE). Para aceptar una contribución hace falta firmar antes el [acuerdo de licencia de contribuidor](CLA.md) (CLA), basado en el Harmony Individual CLA: conservas la autoría de tu código y das al proyecto una licencia sobre él, que le permite publicarlo también con otras licencias.

La firma es automática: en tu primer pull request, el bot del CLA comenta con una frase; respóndela en un comentario del PR y queda registrada. Hasta entonces el PR no se puede fusionar. Si contribuyes en nombre de tu empresa, o parte del código no es tuyo, dilo en el PR antes de firmar.

## Ramas

Todo desarrollo sale de `dev` a una rama propia (`feature/<nombre>`, `bugfix/<nombre>`…) y vuelve a `dev` con un pull request. A `main` solo se llega con un pull request de `dev` a `main`.

## Comprobaciones

Antes de abrir el PR, pasa `make test` (o `ci/test.sh`). Las reglas del proyecto están en [AGENTS.md](AGENTS.md), y la especificación en [PROJECT.md](docs/PROJECT.md).
