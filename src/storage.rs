use crate::model::State;
use anyhow::{Context, Result, bail};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub fn data_path() -> Result<PathBuf> {
    #[cfg(target_os = "android")]
    return crate::android::data_path();
    #[cfg(not(target_os = "android"))]
    {
        let dirs = directories::ProjectDirs::from("fr", "GarageDistiller", "Garage Distiller")
            .context("Impossible de déterminer le dossier de données")?;
        Ok(dirs.data_local_dir().join("state.json"))
    }
}

pub fn load(path: &Path) -> Result<State> {
    if !path.exists() {
        return Ok(State::default());
    }
    let state: State = serde_json::from_slice(&std::fs::read(path)?).context(
        "Données illisibles : le fichier original est conservé, aucune écriture ne sera faite",
    )?;
    if state.schema_version != 1 {
        bail!(
            "Version de données non prise en charge : {}",
            state.schema_version
        );
    }
    Ok(state)
}

pub fn save(path: &Path, state: &State) -> Result<()> {
    let parent = path.parent().context("Chemin de données invalide")?;
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(temp.as_file_mut(), state)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .context("Impossible d’enregistrer les données")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_save_roundtrip_and_reject_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut state = load(&path).unwrap();
        state.settings.scan_minutes = 30;
        save(&path, &state).unwrap();
        state.settings.scan_minutes = 90;
        save(&path, &state).unwrap();
        assert_eq!(load(&path).unwrap().settings.scan_minutes, 90);
        std::fs::write(&path, b"broken").unwrap();
        assert!(load(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
    }
}
