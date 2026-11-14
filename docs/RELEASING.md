# Publier et mettre à jour Garage Distiller

Le dépôt de distribution est [francoispeyret/garage-distiller](https://github.com/francoispeyret/garage-distiller/releases). L'application vérifie les releases stables en arrière-plan et installe uniquement une version plus récente correspondant à son architecture. Elle vérifie la signature Ed25519 de l'archive et sa somme SHA-256 avant remplacement. La nouvelle version prend effet à la prochaine ouverture.

## Configuration initiale, une seule fois

**Dépôt configuré le 14 novembre 2026.** La variable publique et le secret privé ont été créés dans `francoispeyret/garage-distiller`, puis vérifiés. La partie publique est disponible dans `assets/update-public-key.txt`. La clé privée est conservée uniquement dans le secret GitHub Actions ; les fichiers temporaires de génération ont été supprimés et aucune copie privée n'est enregistrée dans les sources ou sur cet ordinateur. Les clés existantes ne doivent pas être remplacées : les versions déjà installées font confiance à cette clé publique.

Le script suivant vérifie une configuration existante sans la modifier. Sur ce dépôt uniquement, si les deux valeurs sont absentes et qu'aucune clé publique locale n'existe, il crée une nouvelle paire, transmet la partie privée par l'entrée standard de `gh secret set`, puis enregistre la partie publique. Il refuse une configuration partielle ou une rotation implicite.

```sh
cargo install zipsign --version 0.2.2 --locked --root .tools
python3 scripts/configure_updates.py
```

Pour une **nouvelle installation sur un autre dépôt**, la procédure manuelle ci-dessous permet de garder une sauvegarde privée hors du dépôt. Ne pas l'exécuter pour remplacer les clés du dépôt déjà configuré.

```sh
cargo install zipsign --version 0.2.2 --locked
zipsign gen-key garage-update.private.key garage-update.public.key
```

Dans GitHub → Settings → Secrets and variables → Actions :

- Variable `GARAGE_DISTILLER_UPDATE_PUBLIC_KEY_HEX` : les 32 octets de `garage-update.public.key`, encodés en 64 caractères hexadécimaux.
- Secret `GARAGE_DISTILLER_UPDATE_PRIVATE_KEY_BASE64` : le contenu intégral de `garage-update.private.key`, encodé en base64 sans retour à la ligne.

Exemple pour produire ces valeurs localement (ne pas les copier dans un ticket ou un commit) :

```sh
python3 -c 'from pathlib import Path; print(Path("garage-update.public.key").read_bytes().hex())'
python3 -c 'from pathlib import Path; import base64; print(base64.b64encode(Path("garage-update.private.key").read_bytes()).decode())'
```

La clé publique est intégrée au binaire à la compilation. Une compilation locale sans la variable `GARAGE_DISTILLER_UPDATE_PUBLIC_KEY_HEX` peut rechercher les versions mais refuse leur installation automatique. Pour compiler localement avec la clé publique de ce dépôt, définir cette variable à partir de `assets/update-public-key.txt` avant `cargo build --release`. Une release GitHub échoue si la clé manque ou si les clés publique et privée ne correspondent pas. Utiliser la même clé pour les versions suivantes ; un changement de clé demande une version de transition et une stratégie de rotation explicite. GitHub ne permet pas de relire la valeur du secret : ne pas supprimer le secret sans procédure de rotation et de continuité préparée.

## Publier une version stable

1. Mettre à jour la version dans `Cargo.toml`, puis régénérer et committer `Cargo.lock`.
2. Vérifier `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings` et `cargo test --locked`.
3. Pousser le code puis un tag correspondant exactement à la version, par exemple `v0.1.0`.
4. Le workflow `Signed desktop release` construit et teste macOS Apple Silicon, macOS Intel et Windows x64. Il prépare les trois archives, les signe, vérifie les signatures, génère `SHA256SUMS`, puis publie la release complète.

Les noms d'archives font partie du protocole : `garage-distiller-vVERSION-TARGET.zip`. Ne pas renommer une archive après signature : son nom est également authentifié. En cas d'échec de publication laissant une release en brouillon, inspecter/supprimer ce brouillon avant de relancer le job. Ne pas remplacer les fichiers d'une release déjà distribuée ; publier une nouvelle version.

## Installation et signature système

- **macOS** : décompresser, déplacer `Garage Distiller.app` dans un dossier accessible en écriture (par exemple `~/Applications`), puis ouvrir l'application. Une mise à jour remplace le bundle entier, y compris ses ressources et sa signature. Le workflow produit une signature ad hoc, pas une signature Developer ID ni une notarisation Apple. Pour une distribution sans avertissement Gatekeeper, ajouter la signature Developer ID et la notarisation avant de signer l'archive de mise à jour. L'application ne contourne pas Gatekeeper ni la translocation macOS.
- **Windows** : décompresser dans un dossier accessible en écriture, puis lancer `garage-distiller.exe`. Le remplacement de l'exécutable en cours d'utilisation est géré par `self_update`/`self-replace`. L'exécutable n'est pas signé Authenticode ; SmartScreen peut donc afficher un avertissement.
- Fermer l'application arrête la surveillance des annonces. Les mises à jour ne relancent pas automatiquement l'application et ne forcent pas sa fermeture.

Les signatures d'archives sécurisent la mise à jour ; elles sont distinctes des signatures de confiance Apple/Microsoft. La chaîne de publication doit être exécutée et les clés configurées avant que les mises à jour puissent être utilisées en production.

## Références

- [API self_update et vérification des signatures](https://docs.rs/self_update/1.3.3/self_update/)
- [Outil zipsign](https://github.com/Kijewski/zipsign)
- [Runners GitHub macOS et Windows](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
