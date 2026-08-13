import { chromium } from "playwright";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { execFile } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { promisify } from "node:util";

const here = dirname(fileURLToPath(import.meta.url));
const output = resolve(here, "../../web/public/screenshots");
const baseUrl = process.env.VELGRINOR_SCREENSHOT_URL || "http://127.0.0.1:1420";
const run = promisify(execFile);
const projects = [
  { id: "AANobbMI", slug: "sodium", name: "Sodium", description: "The fastest and most compatible rendering optimization mod.", icon_url: "https://cdn.modrinth.com/data/AANobbMI/295862f4724dc3f78df3447ad6072b2dcd3ef0c9_96.webp", platform: "modrinth", content_type: "mod", downloads: 206077523, updated: "2026-08-07", categories: ["optimization"], game_versions: ["1.21.1"], loaders: ["fabric"] },
  { id: "YL57xq9U", slug: "iris", name: "Iris Shaders", description: "Modern shader support with excellent performance and compatibility.", icon_url: "https://cdn.modrinth.com/data/YL57xq9U/18d0e7f076d3d6ed5bedd472b853909aac5da202_96.webp", platform: "modrinth", content_type: "mod", downloads: 160378360, updated: "2026-08-03", categories: ["visual"], game_versions: ["1.21.1"], loaders: ["fabric"] },
  { id: "P7dR8mSH", slug: "fabric-api", name: "Fabric API", description: "Essential hooks and interoperability for Fabric mods.", icon_url: "https://cdn.modrinth.com/data/P7dR8mSH/icon.png", platform: "modrinth", content_type: "mod", downloads: 203400000, updated: "2026-08-03", categories: ["library"], game_versions: ["1.21.1"], loaders: ["fabric"] },
  { id: "gvQqBUqZ", slug: "lithium", name: "Lithium", description: "General-purpose optimization without changing vanilla behavior.", icon_url: "https://cdn.modrinth.com/data/gvQqBUqZ/bcc8686c13af0143adf4285d741256af824f70b7_96.webp", platform: "modrinth", content_type: "mod", downloads: 97800000, updated: "2026-08-04", categories: ["optimization"], game_versions: ["1.21.1"], loaders: ["fabric"] },
];
const versions = ["0.6.13", "1.8.8", "0.116.1+1.21.1", "0.15.0"];
const refs = projects.map((project, index) => ({ name: project.name, hash: `hash-${index}`, version: versions[index], source: `https://cdn.modrinth.com/data/${project.id}/versions/demo/${project.id}.jar`, file_name: `${project.id}.jar`, platform: "modrinth", project_id: project.id, version_id: `version-${index}`, enabled: true, pinned: index === 0 }));
const tags = [{ id: 1, name: "mc:1.21.1", color: "#00e07c" }, { id: 2, name: "loader:fabric", color: "#00a331" }, { id: 3, name: "Performance", color: "#007103" }];
const library = refs.map((item, index) => ({ id: index + 1, hash: item.hash, content_type: "mod", name: item.name, file_name: item.file_name, file_size: 1800000 + index * 430000, source_url: item.source, source_platform: item.platform, source_project_id: item.project_id, source_version: item.version, added_at: "2026-08-13T12:00:00Z", updated_at: "2026-08-13T12:00:00Z", notes: null, tags, used_by_profiles: ["Fabulously Optimized"] }));
const profile = { id: "Fabulously Optimized", mcVersion: "1.21.1", loader: { type: "fabric", version: "0.16.14" }, mods: refs, resourcepacks: [{ name: "Fresh Animations", hash: "pack-1", version: "1.9.4", platform: "modrinth", project_id: "fresh-animations", enabled: true }], shaderpacks: [{ name: "Complementary Reimagined", hash: "shader-1", version: "5.5.1", platform: "modrinth", project_id: "complementary-reimagined", enabled: true }], runtime: { java: null, memory: "6G", args: [] } };

await mkdir(output, { recursive: true });
const temporary = await mkdtemp(resolve(tmpdir(), "velgrinor-screenshots-"));
const browser = await chromium.launch({
  headless: true,
  executablePath: process.env.PLAYWRIGHT_CHROMIUM_PATH || undefined,
});
const context = await browser.newContext({ viewport: { width: 1280, height: 760 }, deviceScaleFactor: 2, colorScheme: "dark" });
const page = await context.newPage();
await page.addInitScript(({ profile, projects, library, tags }) => {
  localStorage.clear();
  localStorage.setItem("velgrinor.language", "en");
  const callbacks = new Map();
  const listeners = new Map();
  let callbackId = 1;
  const resolveCommand = (command, args) => {
    if (command === "plugin:event|listen") {
      const handler = Number(args.handler);
      listeners.set(String(args.event), [...(listeners.get(String(args.event)) || []), handler]);
      return handler;
    }
    if (command === "plugin:event|unlisten") return null;
    if (command === "plugin:os|platform") return "linux";
    if (command === "plugin:updater|check") return null;
    if (command === "load_profile_organization_cmd") return { folders: [], ungrouped: [profile.id] };
    if (command === "list_profiles_cmd") return [profile.id];
    if (command === "load_profile_cmd") return profile;
    if (command === "list_accounts_cmd") return { active: "account-1", accounts: [{ uuid: "account-1", username: "Sqrilizz", kind: "offline" }] };
    if (command === "get_config_cmd") return { auto_update_enabled: true, discord_rpc_enabled: true, discord_app_id: "1521208567036645426", download_concurrency: 3, minimize_on_game_start: true, restore_on_game_exit: true, automatic_snapshot_retention: 20 };
    if (command === "fetch_minecraft_versions_cmd") return { versions: [{ id: "1.21.1", type: "release" }] };
    if (command === "fetch_fabric_versions_cmd") return ["0.16.14"];
    if (command === "get_account_info_cmd") return { uuid: "account-1", username: "Sqrilizz", avatar_url: "", body_url: "", skin_url: "", cape_url: "" };
    if (command === "custom_chrome_enabled_cmd") return false;
    if (command === "get_active_session_cmd") return null;
    if (command === "list_downloads_cmd") return [];
    if (command === "store_search_cmd") return projects;
    if (command === "store_get_project_icons_cmd") return Object.fromEntries(projects.map((item) => [item.id, item.icon_url]));
    if (command === "get_auto_update_enabled_cmd" || command === "get_discord_rpc_enabled_cmd") return true;
    if (command === "get_storage_stats_cmd") return { total_bytes: 1929379840, mods_bytes: 48234496, resourcepacks_bytes: 18677760, shaderpacks_bytes: 46137344, skins_bytes: 524288, minecraft_bytes: 1811939328, database_bytes: 409600, unique_items: 6, total_references: 8, deduplication_savings: 28311552 };
    if (command === "detect_java_installations_cmd") return [{ path: "/usr/lib/jvm/java-21-openjdk/bin/java", version: "21.0.8", major: 21, vendor: "OpenJDK", arch: "x86_64", compatible: true }];
    if (command === "library_list_items_cmd") return library;
    if (command === "library_list_tags_cmd") return tags;
    if (command === "library_get_stats_cmd") return { total_items: library.length, mods_count: library.length, modpacks_count: 0, resourcepacks_count: 1, shaderpacks_count: 1, skins_count: 0, total_size: 11796480, tags_count: tags.length };
    if (command === "list_snapshots_cmd" || command === "list_log_files_cmd" || command === "read_logs_cmd") return [];
    return null;
  };
  const internals = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
    invoke: async (command, args = {}) => resolveCommand(command, args),
    transformCallback: (callback, once = false) => {
      const id = callbackId++;
      callbacks.set(id, (payload) => { callback(payload); if (once) callbacks.delete(id); });
      return id;
    },
    unregisterCallback: (id) => callbacks.delete(id),
    runCallback: (id, payload) => callbacks.get(id)?.(payload),
  };
  Object.assign(window, {
    __TAURI_INTERNALS__: internals,
    __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: (_event, id) => callbacks.delete(id) },
    __TAURI_OS_PLUGIN_INTERNALS__: { platform: "linux", family: "unix", os_type: "linux", arch: "x86_64", version: "release", eol: "\n" },
  });
}, { profile, projects, library, tags });
await page.goto(baseUrl, { waitUntil: "networkidle" });
await page.getByRole("heading", { name: profile.id }).waitFor();
const capture = async (name) => {
  await page.waitForFunction(() => [...document.images].every((image) => image.complete && image.naturalWidth > 0));
  const png = resolve(temporary, `${name}.png`);
  const webp = resolve(output, `${name}.webp`);
  await page.screenshot({ path: png, animations: "disabled" });
  await run("magick", [png, "-quality", "92", webp]);
};
await page.locator(".content-tab").filter({ hasText: "Mods" }).click();
await page.getByText("Sodium", { exact: true }).first().waitFor();
await capture("overview");
await page.getByRole("button", { name: "Library", exact: true }).click();
await page.getByText("Sodium", { exact: true }).first().waitFor();
await capture("library");
await page.getByRole("button", { name: "Store", exact: true }).click();
await page.getByText("The fastest and most compatible rendering optimization mod.", { exact: true }).waitFor();
await capture("store");
await page.getByRole("button", { name: "Settings", exact: true }).click();
await page.getByText("Discord Rich Presence", { exact: true }).waitFor();
await capture("settings");
await browser.close();
await run("magick", [resolve(output, "overview.webp"), resolve(here, "../../screenshot.webp")]);
await rm(temporary, { recursive: true, force: true });
