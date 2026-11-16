use semon_push::enrollment::Store;
use std::path::Path;

fn now() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|n| n.as_secs())
        .map_err(|e| e.to_string())
}
pub(super) fn machines(dir: &Path) -> Result<(), String> {
    let mut rows =
        semon_sessions::received_registry::read_machines(dir).map_err(|e| e.to_string())?;
    // Shared names sort together, while every row keeps its canonical ID.
    rows.sort_by(|a, b| a.owner_name.cmp(&b.owner_name).then(a.id.cmp(&b.id)));
    println!("ID\tOWNER NAME\tHOSTNAME\tDIRECTORY");
    for machine in rows {
        let hostname = semon_sessions::read_facts(
            &dir.join("machines")
                .join(&machine.directory)
                .join("facts.json"),
        )
        .map(|f| f.hostname)
        .unwrap_or_default();
        println!(
            "{}\t{}\t{:?}\t{:?}",
            machine.id,
            machine.owner_name.unwrap_or_default(),
            hostname,
            machine.directory
        );
    }
    Ok(())
}

pub(super) fn name(dir: &Path, id: &str, name: Option<&str>) -> Result<(), String> {
    // Opening the registry enforces the receiver owner's existing custody boundary.
    let mut store = Store::open(dir).map_err(|e| e.to_string())?;
    if !store
        .snapshot()
        .map_err(|e| e.to_string())?
        .machines()
        .iter()
        .any(|m| m.id == id)
    {
        return Err(format!(
            "unknown machine ID {id}; use semon receive machines --dir {}",
            dir.display()
        ));
    }
    store
        .name_machine(id, name, now()?)
        .map_err(|e| e.to_string())?;
    println!("Updated name for {id}.");
    Ok(())
}
