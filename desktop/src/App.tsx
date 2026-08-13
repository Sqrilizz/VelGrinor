import { t, useI18n } from "./i18n";
import { useEffect, useCallback, useRef, useState, lazy, Suspense } from "react";
import clsx from "clsx";
import { invoke } from "@tauri-apps/api/core";
import { check } from "@tauri-apps/plugin-updater";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { platform } from "@tauri-apps/plugin-os";

import { useAppStore } from "./store";
import { useOnline } from "./hooks";
import type { LaunchEvent, ContentRef, ContentTab, Profile, LibraryItem, DownloadSnapshot, SessionInfo } from "./types";
import {
  ErrorBoundary,
  Sidebar,
  ProfileView,
  AccountView,
  Toast,
  ConfirmDialog,
  CreateProfileModal,
  CloneProfileModal,
  DiffProfilesModal,
  AddContentModal,
  DeviceCodeModal,
  ProfileJsonModal,
  JavaDownloadModal,
  WindowControls,
} from "./components";
import { formatContentName } from "./utils";
import type { CreateProfileForm } from "./components";

// Lazy load heavy components (three.js/skinview3d)
const StoreView = lazy(() => import("./components/StoreView").then(m => ({ default: m.StoreView })));
const LogsView = lazy(() => import("./components/LogsView").then(m => ({ default: m.LogsView })));
const LibraryView = lazy(() => import("./components/LibraryView").then(m => ({ default: m.LibraryView })));
const SettingsView = lazy(() => import("./components/SettingsView").then(m => ({ default: m.SettingsView })));

const NO_DRAG_SELECTOR = [
  "button",
  "input",
  "select",
  "textarea",
  "a",
  "label",
  "[role='button']",
  "[contenteditable='true']",
  "[data-tauri-drag-region='false']",
  ".modal",
  ".modal-backdrop",
  ".no-drag",
].join(",");

type DiagnosticReport = {
  blocking: boolean;
  issues: Array<{ severity: "warning" | "error"; message: string; evidence: string }>;
};

function App() {
  useI18n();
  const {
    profile,
    selectedProfileId,
    setSelectedProfileId,
    sidebarView,
    setSidebarView,
    activeModal,
    setActiveModal,
    toast,
    launchStatus,
    setLaunchStatus,
    confirmState,
    setConfirmState,
    debugDrag,
    setDebugDrag,
    loadProfiles,
    loadProfile,
    loadAccounts,
    loadConfig,
    loadProfileOrganization,
    notify,
    runAction,
    getActiveAccount,
    activeTab,
  } = useAppStore();

  const isOnline = useOnline();
  const [launchHidden, setLaunchHidden] = useState(false);
  const [currentPlatform, setCurrentPlatform] = useState<string>("");
  const [customChromeEnabled, setCustomChromeEnabled] = useState(false);
  const [downloads, setDownloads] = useState<DownloadSnapshot[]>([]);
  const [session, setSession] = useState<SessionInfo | null>(null);
  const [, setSessionClock] = useState(0);
  const hideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const clearTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const updateCheckRef = useRef(false);

  // Java download modal state
  const [javaDownloadState, setJavaDownloadState] = useState<{
    javaMajor: number;
    mcVersion: string;
  } | null>(null);

  // Detect platform for platform-specific styling
  useEffect(() => {
    const detectedPlatform = platform();
    setCurrentPlatform(detectedPlatform);

    if (detectedPlatform === "windows") {
      setCustomChromeEnabled(true);
    } else if (detectedPlatform === "linux") {
      invoke<boolean>("custom_chrome_enabled_cmd")
        .then(setCustomChromeEnabled)
        .catch(() => setCustomChromeEnabled(false));
    } else {
      setCustomChromeEnabled(false);
    }
  }, []);

  // Content modal state
  const contentKind = useAppStore((s) => s.activeTab);

  // Precache version data for instant dropdowns
  const { precacheMcVersions, precacheFabricVersions, prefetchActiveAccountSkin } = useAppStore();

  // Initial load
  useEffect(() => {
    const loadInitial = async () => {
      // Load organization first to avoid race condition with sync
      // (sync runs when profiles change, so org must be loaded before profiles)
      await loadProfileOrganization();
      await Promise.all([loadProfiles(), loadAccounts(), loadConfig()]);
      // Precache version data and fetch real skin URL in background (don't await - non-blocking)
      void precacheMcVersions();
      void precacheFabricVersions();
      void prefetchActiveAccountSkin();
    };
    void loadInitial();
  }, [loadProfiles, loadAccounts, loadConfig, loadProfileOrganization, precacheMcVersions, precacheFabricVersions, prefetchActiveAccountSkin]);

  // Load profile when selection changes
  useEffect(() => {
    if (!selectedProfileId) {
      useAppStore.setState({ profile: null });
      return;
    }
    void loadProfile(selectedProfileId);
  }, [selectedProfileId, loadProfile]);

  useEffect(() => {
    void invoke("update_discord_rpc_cmd", { profileId: selectedProfileId }).catch(() => undefined);
  }, [selectedProfileId, profile?.mcVersion]);

  // Launch event listener
  useEffect(() => {
    const unlisten = listen<LaunchEvent>("launch-status", (event) => {
      setLaunchStatus(event.payload);
      if (event.payload.session) setSession(event.payload.stage === "done" || event.payload.stage === "error" ? null : event.payload.session);
      if (event.payload.stage === "error") {
        notify(t("Launch failed"), event.payload.message ?? "Unknown error");
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [setLaunchStatus, notify]);

  useEffect(() => {
    void invoke<SessionInfo | null>("get_active_session_cmd").then((active) => {
      setSession(active);
      if (active) setLaunchStatus({ stage: "running", message: "Minecraft is running", progress: 100, session: active });
    });
    void invoke<DownloadSnapshot[]>("list_downloads_cmd").then(setDownloads);
    const unlisten = listen<DownloadSnapshot>("download-progress", ({ payload }) => {
      setDownloads((current) => {
        const next = current.filter((item) => item.request.id !== payload.request.id);
        return [...next, payload];
      });
    });
    return () => { void unlisten.then((dispose) => dispose()); };
  }, [setLaunchStatus]);

  useEffect(() => {
    if (!session) return;
    const timer = setInterval(() => setSessionClock((value) => value + 1), 1000);
    return () => clearInterval(timer);
  }, [session]);

  const controlDownload = (command: string, id: string, status: DownloadSnapshot["status"]) => {
    setDownloads((current) => current.map((item) => item.request.id === id ? { ...item, status } : item));
    void invoke(command, { id }).catch((error) => notify(t("Download action failed"), String(error)));
  };

  // Background app update check (non-blocking)
  useEffect(() => {
    if (!isOnline || updateCheckRef.current) return;
    updateCheckRef.current = true;
    const run = async () => {
      try {
        const update = await check();
        if (update) {
          notify(t("Update available"), `Version ${update.version} is ready to install in Settings → Updates`);
        }
      } catch {
        // Ignore update errors during background check
      }
    };
    void run();
  }, [isOnline, notify]);

  // Auto-hide running banner after a short delay
  useEffect(() => {
    if (!launchStatus) return;

    setLaunchHidden(false);

    if (hideTimerRef.current) {
      clearTimeout(hideTimerRef.current);
      hideTimerRef.current = null;
    }
    if (clearTimerRef.current) {
      clearTimeout(clearTimerRef.current);
      clearTimerRef.current = null;
    }

    if (launchStatus.stage === "running") {
      // Only hide the banner, don't clear status while game is running
      // This preserves the double-click prevention (if (launchStatus) return)
      hideTimerRef.current = setTimeout(() => {
        setLaunchHidden(true);
      }, 3500);
    }

    if (launchStatus.stage === "done") {
      clearTimerRef.current = setTimeout(() => setLaunchStatus(null), 2500);
    }

    // Clear error status after displaying notification, so user can try again
    if (launchStatus.stage === "error") {
      clearTimerRef.current = setTimeout(() => setLaunchStatus(null), 3000);
    }
  }, [launchStatus, setLaunchStatus]);

  useEffect(() => {
    return () => {
      if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
      if (clearTimerRef.current) clearTimeout(clearTimerRef.current);
    };
  }, []);

  // Keyboard shortcuts
  useEffect(() => {
    const handleKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === "d") {
        event.preventDefault();
        setDebugDrag(!debugDrag);
        return;
      }
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "n") {
        event.preventDefault();
        setActiveModal("create");
        return;
      }
      if (event.key === "Escape") {
        if (confirmState) {
          setConfirmState(null);
          return;
        }
        if (activeModal) {
          setActiveModal(null);
          return;
        }
      }
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [activeModal, confirmState, debugDrag, setDebugDrag, setActiveModal, setConfirmState]);

  // Window dragging
  useEffect(() => {
    if (!customChromeEnabled) return;

    const handleMouseDown = async (e: MouseEvent) => {
      if (e.button !== 0) return;

      const target = e.target as HTMLElement;
      const dragRegion = target.closest(".titlebar-drag-region, [data-tauri-drag-region='true']");
      if (!dragRegion) return;
      if (target.closest(NO_DRAG_SELECTOR)) return;
      // Don't interfere with HTML5 drag operations (e.g., sidebar profile reordering)
      if (target.closest("[draggable='true']")) return;

      e.preventDefault();

      try {
        const appWindow = getCurrentWindow();
        if (e.detail === 2) {
          const isMaximized = await appWindow.isMaximized();
          if (isMaximized) {
            await appWindow.unmaximize();
          } else {
            await appWindow.maximize();
          }
        } else {
          await appWindow.startDragging();
        }
      } catch (err) {
        console.debug("Window drag not available:", err);
      }
    };

    document.addEventListener("mousedown", handleMouseDown);
    return () => document.removeEventListener("mousedown", handleMouseDown);
  }, [customChromeEnabled]);

  // Handlers
  const handleCreateProfile = useCallback(async (form: CreateProfileForm) => {
    await runAction(async () => {
      const payload = {
        id: form.id.trim(),
        mc_version: form.mcVersion.trim(),
        loader_type: form.loaderType.trim() || null,
        loader_version: form.loaderVersion.trim() || null,
        java: form.java.trim() || null,
        memory: form.memory.trim() || null,
        args: form.args.trim() || null,
      };
      await invoke<Profile>("create_profile_cmd", { input: payload });
      await loadProfiles();
      setSelectedProfileId(payload.id);
      setActiveModal(null);
    });
  }, [runAction, loadProfiles, setSelectedProfileId, setActiveModal]);

  const handleCloneProfile = useCallback(async (src: string, dst: string) => {
    await runAction(async () => {
      await invoke("clone_profile_cmd", { src, dst });
      await loadProfiles();
      setSelectedProfileId(dst);
      setActiveModal(null);
    });
  }, [runAction, loadProfiles, setSelectedProfileId, setActiveModal]);

  const handleAddContentFromLibrary = useCallback(async (item: LibraryItem) => {
    if (!selectedProfileId) return;
    await runAction(async () => {
      await invoke<Profile>("library_add_to_profile_cmd", {
        profileId: selectedProfileId,
        itemId: item.id,
      });
      await loadProfile(selectedProfileId);
      setActiveModal(null);
      notify(t("Added"), `${formatContentName(item.name)} added to profile`);
    });
  }, [selectedProfileId, runAction, loadProfile, setActiveModal, notify]);

  const handleRemoveContent = useCallback((item: ContentRef) => {
    if (!selectedProfileId) return;
    setConfirmState({
      title: `Remove ${formatContentName(item.name)}?`,
      message: "This removes it from the profile but keeps the stored file.",
      confirmLabel: "Remove",
      tone: "danger",
      onConfirm: async () => {
        setConfirmState(null);
        await runAction(async () => {
          const payload = { profileId: selectedProfileId, target: item.hash };
          if (activeTab === "mods") await invoke("remove_mod_cmd", payload);
          else if (activeTab === "resourcepacks") await invoke("remove_resourcepack_cmd", payload);
          else await invoke("remove_shaderpack_cmd", payload);
          await loadProfile(selectedProfileId);
        });
      },
    });
  }, [selectedProfileId, activeTab, setConfirmState, runAction, loadProfile]);

  const handleLaunch = useCallback(async () => {
    // Prevent double-click race condition - use getState() for synchronous check
    // to avoid stale closure values between renders
    if (useAppStore.getState().launchStatus) return;

    const activeAccount = getActiveAccount();
    if (!selectedProfileId || !activeAccount) {
      notify(t("No account"), "Add an account first.");
      return;
    }

    // Get profile to check MC version
    const currentProfile = useAppStore.getState().profile;
    if (!currentProfile?.mcVersion) {
      notify(t("Invalid profile"), "Profile has no Minecraft version set.");
      return;
    }

    // Check if compatible Java is available
    const mcVersion = currentProfile.mcVersion;
    const compatibleJava = await invoke<string | null>("find_compatible_java_cmd", { mcVersion });

    if (!compatibleJava) {
      // No compatible Java found - get required version and show download modal
      const requiredJava = await invoke<number>("get_required_java_version_cmd", { mcVersion });
      setJavaDownloadState({ javaMajor: requiredJava, mcVersion });
      return;
    }

    let force = false;
    try {
      const report = await invoke<DiagnosticReport>("diagnose_profile_cmd", { profileId: selectedProfileId });
      const errors = report.issues.filter((issue) => issue.severity === "error");
      if (report.blocking || errors.length > 0) {
        notify(
          t("Launch blocked by diagnostics"),
          errors.map((issue) => `${issue.message}: ${issue.evidence}`).join("\n") || t("Resolve the profile errors before launching."),
        );
        return;
      }
      const warnings = report.issues.filter((issue) => issue.severity === "warning");
      if (warnings.length > 0) {
        const accepted = window.confirm(t("Diagnostics found {{count}} warning(s):\n\n{{warnings}}\n\nLaunch anyway?", {
          count: warnings.length,
          warnings: warnings.map((issue) => `${issue.message}: ${issue.evidence}`).join("\n"),
        }));
        if (!accepted) return;
        force = true;
      }
    } catch (err) {
      notify(t("Diagnostics failed"), String(err));
      return;
    }

    setLaunchStatus({ stage: "queued" });

    try {
      await invoke("launch_profile_cmd", {
        profileId: selectedProfileId,
        accountId: activeAccount.uuid,
        force,
      });
      // Status will be updated by launch-status events
    } catch (err) {
      notify(t("Launch failed"), String(err));
      setLaunchStatus(null);
    }
  }, [selectedProfileId, getActiveAccount, notify, setLaunchStatus]);

  const handleOpenInstance = useCallback(async () => {
    if (!selectedProfileId) return;
    try {
      const path = await invoke<string>("instance_path_cmd", { profileId: selectedProfileId });
      try {
        await revealItemInDir(path);
      } catch {
        await openPath(path);
      }
    } catch (err) {
      notify(t("Failed to open folder"), String(err));
    }
  }, [selectedProfileId, notify]);

  const handleCopyCommand = useCallback(async () => {
    if (!selectedProfileId) return;
    const command = `velgrinor launch ${selectedProfileId}`;
    await navigator.clipboard.writeText(command);
    notify(t("Copied"), command);
  }, [selectedProfileId, notify]);

  const handleDeleteProfile = useCallback((id: string) => {
    setConfirmState({
      title: `Delete ${id}?`,
      message: "This will permanently delete the profile and its settings.",
      confirmLabel: "Delete",
      tone: "danger",
      onConfirm: async () => {
        setConfirmState(null);
        await runAction(async () => {
          await invoke("delete_profile_cmd", { id });
          await loadProfiles();
          if (selectedProfileId === id) {
            setSelectedProfileId(null);
          }
        });
      },
    });
  }, [setConfirmState, runAction, loadProfiles, selectedProfileId, setSelectedProfileId]);

  const handleDeviceCodeSuccess = useCallback(async () => {
    await loadAccounts();
  }, [loadAccounts]);

  const openAddContentModal = useCallback((kind: ContentTab) => {
    useAppStore.setState({ activeTab: kind });
    setActiveModal("add-content");
  }, [setActiveModal]);

  return (
    <ErrorBoundary>
      <div
        className={clsx("app-root", debugDrag && "debug-drag")}
        data-platform={currentPlatform || undefined}
      >
        <div className="titlebar-drag-region" />
        <div className="sidebar-titlebar-bg" />
        <WindowControls enabled={customChromeEnabled} />

        {!isOnline && (
          <div
            style={{
              position: "fixed",
              top: 0,
              left: 0,
              right: 0,
              height: 4,
              background: "var(--accent-danger)",
              zIndex: 100,
              opacity: 0.8,
            }}
            title={t("You are offline")}
          />
        )}

        <div className="app-layout">
          <Sidebar
            onCreateProfile={() => setActiveModal("create")}
            onCloneProfile={() => setActiveModal("clone")}
            onDiffProfiles={() => setActiveModal("diff")}
            onAddAccount={() => setActiveModal("device-code")}
            onDeleteProfile={handleDeleteProfile}
          />

          <main className="main-content">
            <div className="content-area">
              <ErrorBoundary>
                {sidebarView === "profiles" && profile && (
                  <ProfileView
                    key={profile.id}
                    onLaunch={handleLaunch}
                    onOpenInstance={handleOpenInstance}
                    onCopyCommand={handleCopyCommand}
                    onShowJson={() => setActiveModal("json")}
                    onAddContent={openAddContentModal}
                    onRemoveContent={handleRemoveContent}
                  />
                )}

                {sidebarView === "profiles" && !profile && (
                  <div className="empty-state">
                    <h3>{t("No profile selected")}</h3>
                    <p>{t("Create your first profile to start launching Minecraft.")}</p>
                    <button className="btn btn-primary" onClick={() => setActiveModal("create")}>{t("Create profile")}</button>
                  </div>
                )}

                {sidebarView === "accounts" && (
                  <AccountView onAddAccount={() => setActiveModal("device-code")} />
                )}

                {sidebarView === "store" && (
                  <Suspense fallback={<div className="loading-view">{t("Loading store...")}</div>}>
                    <StoreView />
                  </Suspense>
                )}

                {sidebarView === "logs" && (
                  <Suspense fallback={<div className="loading-view">{t("Loading logs...")}</div>}>
                    <LogsView />
                  </Suspense>
                )}

                {sidebarView === "library" && (
                  <Suspense fallback={<div className="loading-view">{t("Loading library...")}</div>}>
                    <LibraryView />
                  </Suspense>
                )}

                {sidebarView === "settings" && (
                  <Suspense fallback={<div className="loading-view">{t("Loading settings...")}</div>}>
                    <SettingsView />
                  </Suspense>
                )}
              </ErrorBoundary>
            </div>
          </main>

          {launchStatus && (
            <div className={clsx("launch-status", launchHidden && !session && "is-hidden")}>
              <div className="launch-status-content">
                <div className={`launch-status-dot${launchStatus.stage === "running" ? " is-running" : ""}`} />
                <div className="launch-status-copy">
                  <div className="launch-status-text">
                    {launchStatus.stage.charAt(0).toUpperCase() + launchStatus.stage.slice(1)}
                    {launchStatus.message && `: ${t(launchStatus.message)}`}
                  </div>
                  {launchStatus.stage === "preparing" && launchStatus.progress != null && (
                    <div className="launch-status-track">
                      <div className="launch-status-fill" style={{ width: `${launchStatus.progress}%` }} />
                    </div>
                  )}
                  {session && (
                    <div className="launch-session-meta">
                      {Math.floor((Date.now() / 1000 - session.started_at) / 60)}{t("m ·")} {session.java.split(/[\\/]/).pop()} · {session.ram ?? t("auto RAM")} · {session.gpu ?? t("system GPU")}
                    </div>
                  )}
                </div>
              </div>
              {launchStatus.progress != null && launchStatus.stage === "preparing" && (
                <div className="launch-status-percent">{launchStatus.progress}%</div>
              )}
              {session && (
                <div className="download-tray-actions">
                  <button className="btn btn-ghost btn-sm" onClick={() => setSidebarView("logs")}>{t("Open logs")}</button>
                  <button className="btn btn-danger btn-sm" onClick={() => void invoke("stop_session_cmd", { sessionId: session.session_id })}>{t("Stop game")}</button>
                </div>
              )}
            </div>
          )}
          {downloads.some((item) => !["completed", "cancelled"].includes(item.status)) && (
            <div className="download-tray">
              {downloads.filter((item) => !["completed", "cancelled"].includes(item.status)).map((item, index, activeDownloads) => {
                const percent = item.total ? Math.min(100, Math.round(item.downloaded / item.total * 100)) : 0;
                return <div className="download-tray-item" key={item.request.id}>
                  <div className="download-tray-copy">
                    <strong>{item.request.label ?? item.request.destination.split(/[\\/]/).pop()}</strong>
                    <span>{index + 1}/{activeDownloads.length} · {percent}% · {(item.downloaded / 1048576).toFixed(1)}{t("MB")}{item.total ? ` / ${(item.total / 1048576).toFixed(1)} MB` : ""} · {(item.speed_bytes_per_second / 1048576).toFixed(1)}{t("MB/s")}{item.eta_seconds != null ? ` · ${item.eta_seconds}s` : ""}</span>
                    <div className="launch-status-track"><div className="launch-status-fill" style={{ width: `${percent}%` }} /></div>
                  </div>
                  <div className="download-tray-actions">
                    {item.status === "running" && <button className="btn btn-ghost btn-sm" onClick={() => controlDownload("pause_download_cmd", item.request.id, "paused")}>{t("Pause")}</button>}
                    {item.status === "paused" && <button className="btn btn-ghost btn-sm" onClick={() => controlDownload("resume_download_cmd", item.request.id, "queued")}>{t("Resume")}</button>}
                    {item.status === "failed" && <button className="btn btn-ghost btn-sm" onClick={() => controlDownload("retry_download_cmd", item.request.id, "queued")}>{t("Retry")}</button>}
                    <button className="btn btn-ghost btn-sm" onClick={() => controlDownload("cancel_download_cmd", item.request.id, "cancelled")}>{t("Cancel")}</button>
                  </div>
                </div>;
              })}
            </div>
          )}
        </div>

        {/* Modals */}
        <CreateProfileModal
          open={activeModal === "create"}
          onClose={() => setActiveModal(null)}
          onSubmit={handleCreateProfile}
        />

        <CloneProfileModal
          open={activeModal === "clone"}
          onClose={() => setActiveModal(null)}
          onSubmit={handleCloneProfile}
        />

        <DiffProfilesModal
          open={activeModal === "diff"}
          onClose={() => setActiveModal(null)}
        />

        <AddContentModal
          open={activeModal === "add-content"}
          kind={contentKind}
          onClose={() => setActiveModal(null)}
          onAddFromLibrary={handleAddContentFromLibrary}
        />

        <DeviceCodeModal
          open={activeModal === "device-code"}
          onClose={() => setActiveModal(null)}
          onSuccess={handleDeviceCodeSuccess}
        />

        <ProfileJsonModal
          open={activeModal === "json"}
          profile={profile}
          onClose={() => setActiveModal(null)}
        />

        <JavaDownloadModal
          open={javaDownloadState !== null}
          onClose={() => setJavaDownloadState(null)}
          javaMajor={javaDownloadState?.javaMajor ?? 21}
          mcVersion={javaDownloadState?.mcVersion ?? ""}
          onSuccess={() => {
            setJavaDownloadState(null);
            // Retry launch after Java is installed
            handleLaunch();
          }}
        />

        {confirmState && (
          <ConfirmDialog state={confirmState} onClose={() => setConfirmState(null)} />
        )}

        {toast && <Toast toast={toast} />}
      </div>
    </ErrorBoundary>
  );
}

export default App;
