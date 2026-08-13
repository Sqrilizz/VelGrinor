import { test, expect } from "@playwright/test";

test.describe("Homepage", () => {
  test("should display the hero section", async ({ page }) => {
    await page.goto("/");

    // Check main heading
    await expect(page.getByRole("heading", { level: 1 })).toContainText("VelGrinor");

    // Check that the launcher preview is visible
    await expect(page.locator(".launcher-preview")).toBeVisible();
  });

  test("should have working navigation", async ({ page }) => {
    await page.goto("/");

    // Check docs link exists
    const docsLink = page.getByRole("link", { name: "Documentation" });
    await expect(docsLink).toBeVisible();
  });

  test("should display features section", async ({ page }) => {
    await page.goto("/");

    // Check features heading
    await expect(page.getByText("Why choose VelGrinor?")).toBeVisible();

    // Check feature cards exist (use heading role to be specific)
    await expect(page.getByRole("heading", { name: "Save Disk Space" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Reproducible Profiles" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "CLI-First" })).toBeVisible();
  });
});

test.describe("Documentation", () => {
  test("should load docs page", async ({ page }) => {
    await page.goto("/docs");

    // Wait for page to load
    await page.waitForLoadState("networkidle");

    // Check that we're on a docs page (may redirect or show content)
    await expect(page.locator("body")).toBeVisible();
  });
});
