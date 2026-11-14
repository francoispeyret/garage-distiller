# Garage Distiller pour Android

L’application partage le moteur Rust, les modèles de données, le collecteur et l’interface egui avec les versions ordinateur. Le projet `android/` fournit une activité Java GameActivity, le manifeste, les ressources et l’assemblage Gradle. La bibliothèque Rust est chargée depuis `libgarage_distiller.so`.

## Installer la version Android

Téléchargez [l’APK Android 0.1.1](https://github.com/francoispeyret/garage-distiller/releases/download/android-v0.1.1/garage-distiller-0.1.1-android-arm64.apk) depuis votre téléphone. Les sources correspondantes sont étiquetées `android-v0.1.1`.

La compilation locale produit `dist/android/garage-distiller-0.1.1-android-arm64.apk`.

- Android **8.0 ou ultérieur** (API 26), processeur **ARM64 / arm64-v8a**, OpenGL ES 3.
- Copiez l’APK sur le téléphone, ouvrez-le et autorisez l’installation depuis cette source lorsque Android le demande.
- Ou, avec le débogage USB autorisé : `adb install -r dist/android/garage-distiller-0.1.1-android-arm64.apk`.
- Dans Réglages, utilisez **Tester les notifications** pour accorder l’autorisation sur Android 13 et ultérieur. Après autorisation, appuyez à nouveau pour vérifier la notification.

La première release Android utilise une **clé de signature locale**, conservée dans `.tools/android-signing/local-build.jks`, exclue de Git. Ce n’est pas une publication Google Play. Le mot de passe de cette clé de développement est `android` : utilisez une clé de distribution dédiée avant une publication publique. Conservez la clé : Android exige la même signature pour installer les mises à jour en préservant les données. Une clé différente impose une désinstallation qui efface les données locales.

## Comportement mobile

La navigation se trouve dans le menu supérieur. Sous 700 points de largeur, les annonces s’affichent en cartes, les actions se réorganisent verticalement et les formulaires sont défilables pour rester accessibles avec le clavier ouvert. Le rendu classique reste disponible sur les grands écrans.

Les recherches, favoris, alertes et réglages sont enregistrés dans le stockage privé de l’application, typiquement `/data/user/0/fr.francoispeyret.garagedistiller/files/state.json`. Aucun accès aux fichiers partagés n’est demandé. Les sauvegardes automatiques Android sont désactivées. La désinstallation efface les données ; une mise à jour avec la même signature les conserve.

La surveillance fonctionne **au premier plan**. Passer à une autre application demande l’arrêt de l’exploration ; les requêtes déjà en cours peuvent finir. Aucun service d’arrière-plan, réveil programmé ni garantie d’alertes application fermée n’est fourni. Les limites des moteurs de recherche restent identiques à celles de la version ordinateur.

Les pages des garages s’ouvrent dans le navigateur Android. Les notifications utilisent un canal Android et l’autorisation système. Le remplacement automatique de l’exécutable utilisé sur ordinateur est désactivé : les mises à jour passent par l’installation d’un nouvel APK. La permission Internet et le trafic HTTP sont autorisés pour les sites de garages qui ne proposent pas HTTPS.

## Reconstruire l’APK

Prérequis : Rust stable (au moins 1.88), Python 3, JDK 17, Android SDK et NDK. Le wrapper Gradle 8.13 est fourni et vérifie le SHA-256 de sa distribution.

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
sdkmanager 'platforms;android-35' 'build-tools;35.0.0' 'ndk;28.2.13676358'
# Définissez JAVA_HOME et ANDROID_HOME selon votre installation.
export JAVA_HOME=/chemin/vers/jdk-17
export ANDROID_HOME=/chemin/vers/android-sdk
scripts/build_android.sh
```

Le script compile la bibliothèque ARM64 avec les versions de `Cargo.lock`, assemble un APK release signé, contrôle sa signature et son alignement ZIP sur 16 Ko, puis écrit l’APK et son SHA-256 dans `dist/android/`. Le NDK r28 produit les segments ELF adaptés aux pages de 16 Ko. Les outils téléchargés localement sont dans `.tools/`, exclus de Git.

Pour utiliser une clé de distribution existante, fournissez `ANDROID_KEYSTORE` (chemin absolu), `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` et `ANDROID_KEY_PASSWORD`. Ne commitez pas ces valeurs ni le keystore. Le script ne remplace jamais une clé locale existante.

La version Android est dérivée de `Cargo.toml` ; le code de version vaut `major × 1 000 000 + minor × 1 000 + patch`. Augmentez la version avant une distribution ultérieure.

Pour développer dans Android Studio, ouvrez `android/`. Compilez d’abord les sources Rust avec le script ou `cargo ndk -t arm64-v8a --platform 26 -o android/app/src/main/jniLibs build --locked --release --lib` ; Gradle seul ne recompile pas Rust.

Le workflow `.github/workflows/android.yml` compile et fournit l’APK en artefact sur les pushes et pull requests. Sa clé locale est créée à chaque exécution : ces artefacts servent aux tests et ne constituent pas un canal de mise à jour stable.

## Vérifications

```sh
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo ndk -t arm64-v8a --platform 26 clippy --locked --lib -- -D warnings
scripts/build_android.sh
```

Le pont de saisie dans `src/android.rs` synchronise le texte et les sélections de GameActivity avec egui, car winit 0.30 ne retransmet pas ses événements de texte. Les sélections UTF-16 sont converties en indices de caractères ; les champs numériques utilisent le clavier Android correspondant.

La compilation cible API 35. La validation sur émulateur et les limites restantes de la build livrée sont consignées dans `dist/android/VALIDATION.md`.

Références : [android-activity 0.6.1](https://docs.rs/crate/android-activity/0.6.1), [GameActivity et le clavier](https://developer.android.com/games/agdk/game-activity/migrate-native-activity).
