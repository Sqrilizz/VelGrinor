use crate::content_store::{ContentStore, ContentVersion, Platform};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyType {
    Required,
    Optional,
    Incompatible,
    Embedded,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentDependency {
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    pub dependency_type: DependencyType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedContent {
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
    pub dependency_type: DependencyType,
    pub requested: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallPlan {
    pub install: Vec<PlannedContent>,
    pub optional: Vec<PlannedContent>,
    pub embedded: Vec<PlannedContent>,
    pub tools: Vec<PlannedContent>,
    pub incompatible: Vec<PlannedContent>,
    pub warnings: Vec<String>,
}

pub trait DependencyCatalog {
    fn dependencies(
        &self,
        project_id: &str,
        version_id: Option<&str>,
    ) -> Result<Vec<ContentDependency>>;
}

pub struct ResolvedInstallPlan {
    pub plan: InstallPlan,
    pub versions: Vec<ContentVersion>,
}

pub fn resolve_store_install_plan(
    store: &ContentStore,
    platform: Platform,
    root: ContentVersion,
    minecraft: &str,
    loader: Option<&str>,
    installed: &HashMap<String, Option<String>>,
) -> Result<ResolvedInstallPlan> {
    let mut plan = InstallPlan::default();
    let mut versions = Vec::new();
    let mut selected = HashMap::<String, String>::new();
    let mut queue = vec![(root, true)];
    while let Some((version, requested)) = queue.pop() {
        if let Some(existing) = selected.get(&version.project_id) {
            if existing != &version.id {
                bail!(
                    "conflicting exact versions for {}: {} and {}",
                    version.project_id,
                    existing,
                    version.id
                );
            }
            continue;
        }
        if !requested && let Some(existing) = installed.get(&version.project_id) {
            ensure_compatible(&version.project_id, existing, &Some(version.id.clone()))?;
            continue;
        }
        selected.insert(version.project_id.clone(), version.id.clone());
        let item = PlannedContent {
            project_id: version.project_id.clone(),
            version_id: Some(version.id.clone()),
            dependency_type: DependencyType::Required,
            requested,
        };
        push_unique(&mut plan.install, item);
        for dependency in &version.dependencies {
            let dependency_type = match dependency.dependency_type.as_str() {
                "required" => DependencyType::Required,
                "optional" => DependencyType::Optional,
                "incompatible" => DependencyType::Incompatible,
                "embedded" => DependencyType::Embedded,
                "tool" => DependencyType::Tool,
                _ => continue,
            };
            let planned = PlannedContent {
                project_id: dependency.project_id.clone(),
                version_id: dependency.version_id.clone(),
                dependency_type,
                requested: false,
            };
            match dependency_type {
                DependencyType::Optional => push_unique(&mut plan.optional, planned),
                DependencyType::Embedded => push_unique(&mut plan.embedded, planned),
                DependencyType::Tool => push_unique(&mut plan.tools, planned),
                DependencyType::Incompatible => {
                    if installed.contains_key(&dependency.project_id)
                        || selected.contains_key(&dependency.project_id)
                    {
                        bail!(
                            "{} is incompatible with {}",
                            version.project_id,
                            dependency.project_id
                        );
                    }
                    push_unique(&mut plan.incompatible, planned);
                }
                DependencyType::Required => {
                    let candidates = store.get_versions(
                        platform,
                        &dependency.project_id,
                        Some(minecraft),
                        loader,
                    )?;
                    let child = if let Some(exact) = dependency.version_id.as_deref() {
                        candidates
                            .into_iter()
                            .find(|candidate| candidate.id == exact)
                            .with_context(|| {
                                format!("exact dependency version not found: {exact}")
                            })?
                    } else {
                        candidates.into_iter().next().with_context(|| {
                            format!("no compatible dependency found: {}", dependency.project_id)
                        })?
                    };
                    queue.push((child, false));
                }
            }
        }
        versions.push(version);
    }
    plan.install.sort_by(|a, b| a.project_id.cmp(&b.project_id));
    Ok(ResolvedInstallPlan { plan, versions })
}

pub fn resolve_install_plan<C: DependencyCatalog>(
    catalog: &C,
    requested: &[ContentDependency],
    installed: &HashMap<String, Option<String>>,
) -> Result<InstallPlan> {
    let mut plan = InstallPlan::default();
    let mut selected = HashMap::<String, Option<String>>::new();
    let mut expanded = HashSet::<(String, Option<String>)>::new();
    let mut stack = requested
        .iter()
        .cloned()
        .map(|dependency| (dependency, true))
        .collect::<Vec<_>>();

    while let Some((dependency, is_requested)) = stack.pop() {
        if dependency.dependency_type == DependencyType::Incompatible {
            if installed.contains_key(&dependency.project_id)
                || selected.contains_key(&dependency.project_id)
            {
                bail!("incompatible dependency present: {}", dependency.project_id);
            }
            push_unique(&mut plan.incompatible, planned(&dependency, is_requested));
            continue;
        }

        let bucket = match dependency.dependency_type {
            DependencyType::Optional => &mut plan.optional,
            DependencyType::Embedded => &mut plan.embedded,
            DependencyType::Tool => &mut plan.tools,
            DependencyType::Required => &mut plan.install,
            DependencyType::Incompatible => unreachable!(),
        };
        push_unique(bucket, planned(&dependency, is_requested));

        if dependency.dependency_type != DependencyType::Required {
            continue;
        }

        if let Some(existing) = installed.get(&dependency.project_id) {
            ensure_compatible(&dependency.project_id, existing, &dependency.version_id)?;
            plan.install
                .retain(|item| item.project_id != dependency.project_id);
            continue;
        }
        if let Some(existing) = selected.get(&dependency.project_id) {
            ensure_compatible(&dependency.project_id, existing, &dependency.version_id)?;
        } else {
            selected.insert(dependency.project_id.clone(), dependency.version_id.clone());
        }

        let key = (dependency.project_id.clone(), dependency.version_id.clone());
        if !expanded.insert(key) {
            continue;
        }
        for child in
            catalog.dependencies(&dependency.project_id, dependency.version_id.as_deref())?
        {
            if child.dependency_type == DependencyType::Incompatible
                && (installed.contains_key(&child.project_id)
                    || selected.contains_key(&child.project_id))
            {
                bail!(
                    "{} is incompatible with {}",
                    dependency.project_id,
                    child.project_id
                );
            }
            stack.push((child, false));
        }
    }

    plan.install.sort_by(|a, b| a.project_id.cmp(&b.project_id));
    plan.optional
        .sort_by(|a, b| a.project_id.cmp(&b.project_id));
    Ok(plan)
}

fn ensure_compatible(
    project: &str,
    current: &Option<String>,
    wanted: &Option<String>,
) -> Result<()> {
    if let (Some(current), Some(wanted)) = (current, wanted)
        && current != wanted
    {
        bail!("conflicting exact versions for {project}: {current} and {wanted}");
    }
    Ok(())
}

fn planned(dependency: &ContentDependency, requested: bool) -> PlannedContent {
    PlannedContent {
        project_id: dependency.project_id.clone(),
        version_id: dependency.version_id.clone(),
        dependency_type: dependency.dependency_type,
        requested,
    }
}

fn push_unique(items: &mut Vec<PlannedContent>, item: PlannedContent) {
    if !items
        .iter()
        .any(|existing| existing.project_id == item.project_id)
    {
        items.push(item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Catalog(HashMap<String, Vec<ContentDependency>>);

    impl DependencyCatalog for Catalog {
        fn dependencies(
            &self,
            project_id: &str,
            _: Option<&str>,
        ) -> Result<Vec<ContentDependency>> {
            Ok(self.0.get(project_id).cloned().unwrap_or_default())
        }
    }

    fn dependency(project: &str, version: &str) -> ContentDependency {
        ContentDependency {
            project_id: project.to_string(),
            version_id: Some(version.to_string()),
            dependency_type: DependencyType::Required,
            source: None,
        }
    }

    #[test]
    fn resolves_cycles_and_diamonds_once() {
        let catalog = Catalog(HashMap::from([
            (
                "root".into(),
                vec![dependency("left", "1"), dependency("right", "1")],
            ),
            ("left".into(), vec![dependency("shared", "1")]),
            ("right".into(), vec![dependency("shared", "1")]),
            ("shared".into(), vec![dependency("root", "1")]),
        ]));
        let plan =
            resolve_install_plan(&catalog, &[dependency("root", "1")], &HashMap::new()).unwrap();
        assert_eq!(plan.install.len(), 4);
    }

    #[test]
    fn rejects_conflicting_exact_versions() {
        let catalog = Catalog(HashMap::from([
            (
                "root".into(),
                vec![dependency("shared", "1"), dependency("other", "1")],
            ),
            ("other".into(), vec![dependency("shared", "2")]),
        ]));
        assert!(
            resolve_install_plan(&catalog, &[dependency("root", "1")], &HashMap::new()).is_err()
        );
    }

    #[test]
    fn reuses_compatible_installed_dependency() {
        let catalog = Catalog(HashMap::new());
        let installed = HashMap::from([("shared".to_string(), Some("1".to_string()))]);
        let plan =
            resolve_install_plan(&catalog, &[dependency("shared", "1")], &installed).unwrap();
        assert!(plan.install.is_empty());
    }
}
