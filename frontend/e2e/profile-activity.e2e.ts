import { expect, test } from "@playwright/test";

test("activity hover details and repository card shortcuts", async ({
  page,
}) => {
  test.setTimeout(90_000);
  const username = process.env.GITADEL_E2E_USERNAME ?? "e2e-admin";
  await page.goto("/login");
  await page.getByLabel("Username").fill(username);
  await page
    .getByLabel("Password", { exact: true })
    .fill(process.env.GITADEL_E2E_PASSWORD ?? "e2e-password-123456");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).not.toHaveURL(/\/login(?:\?|$)/);
  const namespace = `activity-${Date.now()}`;
  const name = "hover-fixture";
  const api = `/api/v1/repositories/${namespace}/${name}`;
  expect(
    (
      await page.request.post("/api/v1/organizations", {
        data: { slug: namespace, display_name: "Activity fixture" },
      })
    ).status(),
  ).toBe(201);
  try {
    expect(
      (
        await page.request.post("/api/v1/repositories", {
          data: {
            namespace,
            name,
            visibility: "private",
            object_format: "sha1",
          },
        })
      ).status(),
    ).toBe(201);
    expect(
      (
        await page.request.post(`${api}/files`, {
          data: {
            branch: "main",
            expected_commit: null,
            path: "demo.mjs",
            content: "export const demo = true;\n",
            message: "Add activity fixture",
          },
        })
      ).status(),
    ).toBe(200);
    await page.goto(`/${namespace}`);
    const today = new Date().toISOString().slice(0, 10);
    const day = page.locator(`[data-contribution-date="${today}"]`);
    await expect(day).toHaveAttribute("aria-label", /^1 commit on/);
    let detailRequests = 0;
    page.on("request", (request) => {
      if (request.url().endsWith(`/namespaces/${namespace}/activity/${today}`))
        detailRequests++;
    });
    await day.hover();
    const popover = page.locator('[data-slot="hover-card-content"]');
    await expect(popover).toBeVisible();
    await expect(popover.getByText("1 commit", { exact: true })).toBeVisible();
    await expect(
      popover.getByRole("link", { name: `${namespace}/${name}`, exact: true }),
    ).toBeVisible();
    await expect(popover.getByText("Across 1 repository")).toBeVisible();
    await page
      .getByRole("heading", { name: "Activity fixture", exact: true })
      .hover();
    await expect(popover).toBeHidden();
    await day.hover();
    await expect(popover.getByText("Across 1 repository")).toBeVisible();
    expect(detailRequests).toBe(1);
    await day.focus();
    await page.keyboard.press("ArrowUp");
    await expect(day).not.toBeFocused();
    const previous = new Date(`${today}T00:00:00Z`);
    previous.setUTCDate(previous.getUTCDate() - 1);
    const previousDay = page.locator(
      `[data-contribution-date="${previous.toISOString().slice(0, 10)}"]`,
    );
    await expect(previousDay).toBeFocused();
    await previousDay.hover();
    await expect(
      popover.getByText("No commits recorded on this day."),
    ).toBeVisible();

    const favorite = page.getByRole("button", {
      name: `Favorite ${namespace}/${name}`,
      exact: true,
    });
    await expect(favorite).toBeVisible();
    const card = favorite.locator("..");
    const zip = card.getByRole("link", {
      name: `Download ZIP source for ${namespace}/${name}`,
      exact: true,
    });
    const tar = card.getByRole("link", {
      name: `Download TAR.GZ source for ${namespace}/${name}`,
      exact: true,
    });
    for (const [link, format, contentType] of [
      [zip, "zip", "application/zip"],
      [tar, "tar.gz", "application/gzip"],
    ] as const) {
      const href = await link.getAttribute("href");
      expect(href).toContain(`format=${format}`);
      const response = await page.request.get(href!);
      expect(response.status()).toBe(200);
      expect(response.headers()["content-type"]).toContain(contentType);
    }
    const favoriteBox = await favorite.boundingBox();
    const zipBox = await zip.boundingBox();
    expect(favoriteBox!.x).toBeLessThan(zipBox!.x);
    await favorite.click();
    await expect(
      page.getByRole("button", {
        name: `Unfavorite ${namespace}/${name}`,
        exact: true,
      }),
    ).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`/${namespace}$`));
  } finally {
    await page.request.post(`${api}/delete`);
    await page.request.delete(`${api}/purge`);
    await page.request.delete(`/api/v1/organizations/${namespace}`);
  }
});
