import { expect, test, type Page } from "@playwright/test";

type Scenario = "base" | "downloads" | "diagnostics" | "crash" | "session";
type BrowserIssue = { source: "console" | "page"; message: string };

const installTauriMock = async (page: Page, scenario: Scenario) => {
  await page.addInitScript(({ scenarioName }) => {
    const callbacks = new Map<number, (payload: unknown) => void>();
    const listeners = new Map<string, number[]>();
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    let callbackId = 1;
    let snapshots = [{ id: "automatic-1", created_at: 1710000000, automatic: true, reason: "before launch", size: 4096 }];
    const profile = {
      id: "Test Profile",
      mcVersion: "1.21.1",
      loader: { type: "fabric", version: "0.16.9" },
      mods: [],
      resourcepacks: [],
      shaderpacks: [],
      runtime: { java: null, memory: "4G", args: [] },
    };
    const download = {
      request: { id: "download-1", url: "https://example.test/mod.jar", destination: "/tmp/mod.jar", label: "Sodium" },
      status: "running",
      downloaded: 5242880,
      total: 10485760,
      speed_bytes_per_second: 1048576,
      eta_seconds: 5,
      attempts: 1,
      error: null,
    };
    const session = {
      session_id: "session-1",
      pid: 4242,
      profile_id: "Test Profile",
      java: "/usr/bin/java",
      ram: "4G",
      gpu: "Mock GPU",
      started_at: Math.floor(Date.now() / 1000) - 120,
    };
    const resolveCommand = (command: string, args: Record<string, unknown>) => {
      if (command === "plugin:event|listen") {
        const event = String(args.event);
        const handler = Number(args.handler);
        listeners.set(event, [...(listeners.get(event) ?? []), handler]);
        return handler;
      }
      if (command === "plugin:event|unlisten") return null;
      if (command === "plugin:os|platform") return "linux";
      if (command === "plugin:updater|check") return null;
      if (command === "load_profile_organization_cmd") return { folders: [], ungrouped: [profile.id] };
      if (command === "list_profiles_cmd") return [profile.id];
      if (command === "load_profile_cmd") return profile;
      if (command === "list_accounts_cmd") return { active: "account-1", accounts: [{ uuid: "account-1", username: "Player", kind: "offline" }] };
      if (command === "get_config_cmd") return { download_concurrency: 3, minimize_on_game_start: true, restore_on_game_exit: true, automatic_snapshot_retention: 20 };
      if (command === "fetch_minecraft_versions_cmd") return { versions: [{ id: "1.21.1", type: "release" }] };
      if (command === "fetch_fabric_versions_cmd") return ["0.16.9"];
      if (command === "get_account_info_cmd") return { uuid: "account-1", username: "Player", avatar_url: "", body_url: "", skin_url: "", cape_url: "" };
      if (command === "custom_chrome_enabled_cmd") return false;
      if (command === "get_active_session_cmd") return scenarioName === "session" ? session : null;
      if (command === "list_downloads_cmd") return scenarioName === "downloads" ? [download] : [];
      if (command === "store_search_cmd" || command === "store_get_versions_cmd") return [];
      if (command === "ogulniega_list_cmd") return [{
        name: "1.21.1-sodium",
        minecraft_version: "1.21.1",
        fabric_version: "0.19.3",
        loader_name: "fabric-1.21.1-0.17.3",
        java_name: "jdk-21",
        jvm_args: ["-Dfabric.addMods=mods/1.21.1-sodium"],
      }];
      if (command === "ogulniega_install_cmd") return { ...profile, id: "ogulniega-1.21.1-sodium" };
      if (command === "get_auto_update_enabled_cmd") return true;
      if (command === "get_storage_stats_cmd") return { total_bytes: 0, mods_bytes: 0, resourcepacks_bytes: 0, shaderpacks_bytes: 0, skins_bytes: 0, minecraft_bytes: 0, database_bytes: 0 };
      if (command === "detect_java_installations_cmd") return [];
      if (command === "diagnose_profile_cmd") return { blocking: true, issues: [{ code: "missing", severity: "error", message: "Missing dependency", evidence: "fabric-api.jar is absent", fix: { type: "add_dependency", project: "fabric-api" } }] };
      if (command === "apply_diagnostic_fix_cmd") return { blocking: false, issues: [] };
      if (command === "list_snapshots_cmd") return snapshots;
      if (command === "create_snapshot_cmd") {
        snapshots = [...snapshots, { id: "manual-1", created_at: 1710000100, automatic: false, reason: "manual snapshot", size: 2048 }];
        return snapshots[1];
      }
      if (command === "list_log_files_cmd" || command === "read_logs_cmd") return [];
      if (command === "library_list_items_cmd" || command === "library_list_tags_cmd") return [];
      if (command === "library_get_stats_cmd") return { total_items: 0, mods_count: 0, modpacks_count: 0, resourcepacks_count: 0, shaderpacks_count: 0, skins_count: 0, total_size: 0, tags_count: 0 };
      if (command === "list_crash_reports_cmd") return [{ name: "crash-2026-08-13.txt", path: "/tmp/crash.txt", size: 1200, modified: 1710000000, is_current: true }];
      if (command === "analyze_last_crash_cmd") return { category: "memory", probable_cause: "Java heap exhausted", evidence: ["java.lang.OutOfMemoryError: Java heap space"], actions: ["Increase RAM", "Disable suspect mod"], exit_code: 1 };
      return null;
    };
    const internals = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
      invoke: async (command: string, args: Record<string, unknown> = {}) => {
        calls.push({ command, args });
        return resolveCommand(command, args);
      },
      transformCallback: (callback: (payload: unknown) => void, once = false) => {
        const id = callbackId++;
        callbacks.set(id, (payload) => {
          callback(payload);
          if (once) callbacks.delete(id);
        });
        return id;
      },
      unregisterCallback: (id: number) => callbacks.delete(id),
      runCallback: (id: number, payload: unknown) => callbacks.get(id)?.(payload),
    };
    Object.assign(window, {
      __TAURI_INTERNALS__: internals,
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: (_event: string, id: number) => callbacks.delete(id) },
      __TAURI_OS_PLUGIN_INTERNALS__: { platform: "linux", family: "unix", os_type: "linux", arch: "x86_64", version: "test", eol: "\n" },
      __mockCalls: calls,
      __emitTauri: (event: string, payload: unknown) => {
        for (const handler of listeners.get(event) ?? []) callbacks.get(handler)?.({ event, id: handler, payload });
      },
    });
  }, { scenarioName: scenario });
};

const openApp = async (page: Page, scenario: Scenario) => {
  const issues: BrowserIssue[] = [];
  page.on("pageerror", (error) => issues.push({ source: "page", message: error.message }));
  page.on("console", (message) => {
    if (message.type() === "error") issues.push({ source: "console", message: message.text() });
  });
  await page.route("https://mc-heads.net/**", (route) => route.fulfill({
    status: 200,
    contentType: "image/svg+xml",
    body: '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"/>',
  }));
  await page.route("https://ogulniega.com/**", (route) => route.fulfill({
    status: 200,
    contentType: "image/svg+xml",
    body: '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"/>',
  }));
  await installTauriMock(page, scenario);
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Test Profile" })).toBeVisible();
  return issues;
};

const expectNoBrowserIssues = async (page: Page, issues: BrowserIssue[]) => {
  await page.waitForTimeout(50);
  expect(issues).toEqual([]);
};

test("primary screens render without browser errors", async ({ page }) => {
  const issues = await openApp(page, "base");

  await page.getByRole("button", { name: "Library", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Library is empty" })).toBeVisible();

  await page.getByRole("button", { name: "Store", exact: true }).click();
  await expect(page.getByPlaceholder("Search mods...")).toBeVisible();

  await page.getByRole("button", { name: "Logs", exact: true }).click();
  await expect(page.getByRole("heading", { name: "No logs yet" })).toBeVisible();

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByText("Auto-check for updates", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: /Player/ }).click();
  await expect(page.getByRole("heading", { name: "Account" })).toBeVisible();
  await expectNoBrowserIssues(page, issues);
});

test("download tray exposes progress and controls", async ({ page }) => {
  const issues = await openApp(page, "downloads");
  const tray = page.locator(".download-tray");
  await expect(tray).toContainText("Sodium");
  await expect(tray).toContainText("50%");
  await expect(tray).toContainText("1.0MB/s");
  await tray.getByRole("button", { name: "Pause" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __mockCalls: Array<{ command: string }> }).__mockCalls.some((call) => call.command === "pause_download_cmd"))).toBe(true);
  await expectNoBrowserIssues(page, issues);
});

test("Ogulniega builds are offered as a separate install source", async ({ page }) => {
  const issues = await openApp(page, "base");
  await page.getByRole("button", { name: "Store", exact: true }).click();
  await page.getByRole("button", { name: "Modpacks", exact: true }).click();
  await page.locator("select").selectOption("ogulniega");
  await expect(page.getByText("Ogulniega 1.21.1-sodium", { exact: true })).toBeVisible();
  await expect(page.getByText("Minecraft 1.21.1 · Fabric 0.17.3 · Sodium", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Install as a new profile" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __mockCalls: Array<{ command: string; args: Record<string, unknown> }> }).__mockCalls.some((call) => call.command === "ogulniega_install_cmd" && (call.args.input as { pack_name: string }).pack_name === "1.21.1-sodium"))).toBe(true);
  await expectNoBrowserIssues(page, issues);
});

test("diagnostics supports fixes and manual snapshots", async ({ page }) => {
  const issues = await openApp(page, "diagnostics");
  await page.getByRole("button", { name: "Diagnostics", exact: true }).click();
  const panel = page.locator(".reliability-panel");
  await expect(panel).toContainText("Missing dependency");
  await expect(panel).toContainText("before launch");
  await panel.getByRole("button", { name: "Fix" }).click();
  await expect(panel).toContainText("No problems found");
  await panel.getByRole("button", { name: "Create snapshot" }).click();
  await expect(panel).toContainText("manual snapshot");
  await expectNoBrowserIssues(page, issues);
});

test("crash assistant presents local evidence and actions", async ({ page }) => {
  const issues = await openApp(page, "crash");
  await page.getByRole("button", { name: "Logs", exact: true }).click();
  await page.getByRole("button", { name: /Crashes/ }).click();
  await expect(page.getByText("crash-2026-08-13.txt")).toBeVisible();
  await page.getByRole("button", { name: "Analyze latest crash" }).click();
  const analysis = page.locator(".crash-analysis-panel");
  await expect(analysis).toContainText("Java heap exhausted");
  await expect(analysis).toContainText("OutOfMemoryError");
  await expect(analysis).toContainText("Increase RAM");
  await expectNoBrowserIssues(page, issues);
});

test("active session can only be stopped with its session id", async ({ page }) => {
  const issues = await openApp(page, "session");
  const status = page.locator(".launch-status");
  await expect(status).toContainText("Minecraft is running");
  await expect(status).toContainText("java");
  await expect(status).toContainText("4G");
  await expect(status).toContainText("Mock GPU");
  await status.getByRole("button", { name: "Stop game" }).click();
  const stopCall = await page.evaluate(() => (window as unknown as { __mockCalls: Array<{ command: string; args: Record<string, unknown> }> }).__mockCalls.find((call) => call.command === "stop_session_cmd"));
  expect(stopCall?.args.sessionId).toBe("session-1");
  await expectNoBrowserIssues(page, issues);
});
