fn main() {
    println!("cargo:rerun-if-env-changed=VELGRINOR_MS_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=VELGRINOR_MS_CLIENT_SECRET");
    println!("cargo:rerun-if-env-changed=VELGRINOR_CURSEFORGE_API_KEY");
    println!("cargo:rerun-if-env-changed=SHARD_MS_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=VELGRINOR_DISCORD_APP_ID");
    println!("cargo:rerun-if-env-changed=SHARD_MS_CLIENT_SECRET");
    println!("cargo:rerun-if-env-changed=SHARD_CURSEFORGE_API_KEY");
}
