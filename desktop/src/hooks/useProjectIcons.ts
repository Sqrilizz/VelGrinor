import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

const iconCache = new Map<string, string | null>();

export function getModrinthProjectId(item: {
  platform?: string | null;
  source_platform?: string | null;
  project_id?: string | null;
  source_project_id?: string | null;
  source?: string | null;
  source_url?: string | null;
}): string | null {
  const source = item.source || item.source_url;
  const inferred = source?.includes("cdn.modrinth.com/data/")
    ? source.split("cdn.modrinth.com/data/")[1]?.split("/")[0]
    : null;
  const platform = (item.platform || item.source_platform || (inferred ? "modrinth" : null))?.toLowerCase();
  if (platform !== "modrinth") return null;
  return item.project_id || item.source_project_id || inferred || null;
}

export function useProjectIcons(projectIds: Array<string | null | undefined>): Record<string, string> {
  const cacheKey = useMemo(
    () => [...new Set(projectIds.filter((id): id is string => Boolean(id)))].sort().join("|"),
    [projectIds]
  );
  const ids = useMemo(() => cacheKey ? cacheKey.split("|") : [], [cacheKey]);
  const [icons, setIcons] = useState<Record<string, string>>({});

  useEffect(() => {
    let active = true;
    const publish = () => {
      if (!active) return;
      const resolved: Record<string, string> = {};
      for (const id of ids) {
        const icon = iconCache.get(id);
        if (icon) resolved[id] = icon;
      }
      setIcons(resolved);
    };
    const missing = ids.filter((id) => !iconCache.has(id));
    publish();
    if (missing.length === 0) return () => { active = false; };
    void invoke<Record<string, string>>("store_get_project_icons_cmd", { projectIds: missing })
      .then((result) => {
        for (const id of missing) iconCache.set(id, result[id] ?? null);
        publish();
      })
      .catch(() => {
        for (const id of missing) iconCache.set(id, null);
        publish();
      });
    return () => { active = false; };
  }, [cacheKey]);

  return icons;
}
