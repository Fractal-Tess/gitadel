import { expect, test } from "@playwright/test";

test.use({ actionTimeout: 10_000 });

const username = process.env.GITADEL_E2E_USERNAME ?? "e2e-admin";
const password = process.env.GITADEL_E2E_PASSWORD ?? "e2e-password-123456";

test("file navigation, sharing, websites, and narrow sidebars", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/login");
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).not.toHaveURL(/\/login(?:\?|$)/);
  const name = `file-viewer-${Date.now()}`;
  const repository = `${username}/${name}`;
  const api = `/api/v1/repositories/${repository}`;
  expect(
    (
      await page.request.post("/api/v1/repositories", {
        data: {
          namespace: username,
          name,
          visibility: "private",
          object_format: "sha1",
        },
      })
    ).status(),
  ).toBe(201);
  try {
    const source =
      [
        "export function greet() {",
        '  return "hello";',
        "}",
        ...Array.from({ length: 1102 }, (_, i) => `// line ${i + 4}`),
      ].join("\n") + "\n";
    const create = await page.request.post(`${api}/files`, {
      data: {
        branch: "main",
        expected_commit: null,
        path: "src/demo.ts",
        content: source,
        message: "Add source",
      },
    });
    expect(create.status()).toBe(200);
    const first = (await create.json()).commit as string;
    const update = await page.request.put(`${api}/files`, {
      data: {
        branch: "main",
        expected_commit: first,
        path: "src/demo.ts",
        content: source.replace('"hello"', '"updated"'),
        message: "Update greeting",
      },
    });
    expect(update.status()).toBe(200);
    const second = (await update.json()).commit as string;
    expect(
      (
        await page.request.post(`${api}/files`, {
          data: {
            branch: "main",
            expected_commit: second,
            path: "README.md",
            content: "# Demo\n\nRead me.\n\n| Feature | Status |\n| --- | --- |\n| Tables | Rendered |\n",
            message: "Add README",
          },
        })
      ).status(),
    ).toBe(200);
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.goto(`/${repository}?path=src%2Fdemo.ts`);
    await expect(page.getByText("1,105 lines", { exact: true })).toBeVisible();
    await expect(page.locator("[data-code-line]")).toHaveCount(1000);
    await page
      .getByRole("button", { name: "Select line 2", exact: true })
      .click();
    await page
      .getByRole("button", { name: "Select line 3", exact: true })
      .click({ modifiers: ["Shift"] });
    await expect(page).toHaveURL(/#L2-L3$/);
    await page.getByRole("button", { name: "Copy link", exact: true }).click();
    const link = await page.evaluate(() => navigator.clipboard.readText());
    expect(new URL(link).searchParams.get("rev")).toHaveLength(40);
    expect(new URL(link).hash).toBe("#L2-L3");
    await page.getByRole("button", { name: "Copy code", exact: true }).click();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
      '  return "updated";\n}',
    );
    await page
      .getByRole("button", { name: "Create issue", exact: true })
      .click();
    await expect(
      page.getByRole("dialog").getByLabel("Title", { exact: true }),
    ).toHaveValue("Review src/demo.ts:2-3");
    await expect(
      page.getByRole("dialog").getByLabel("Description", { exact: true }),
    ).toHaveValue(/updated/);
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Cancel", exact: true })
      .click();
    await page.goto(link);
    await expect(
      page.getByRole("button", { name: "Select line 2", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await page.getByRole("button", { name: "Outline", exact: true }).click();
    await expect(
      page
        .getByRole("complementary", { name: "File outline" })
        .getByRole("button", { name: /greet/ }),
    ).toBeVisible();
    await page.getByRole("radio", { name: "Blame", exact: true }).click();
    await expect(page.getByText("Loading line history…")).not.toBeVisible();
    await expect(page.locator('[data-code-line="2"]')).toContainText(
      second.slice(0, 7),
    );
    await page.getByRole("radio", { name: "Source", exact: true }).click();
    await page.getByRole("button", { name: "Go to line", exact: true }).click();
    await page.getByLabel("Line or range", { exact: true }).fill("1100");
    await page.getByRole("button", { name: "Go", exact: true }).click();
    await expect(
      page.getByRole("button", { name: "Select line 1100", exact: true }),
    ).toBeVisible();
    await page
      .getByRole("textbox", { name: "Find in file", exact: true })
      .fill("updated");
    await expect(page.locator('[data-code-line="2"] mark')).toHaveText(
      "updated",
    );
    await page.getByRole("button", { name: "Wrap", exact: true }).click();
    await page.reload();
    await expect(
      page.getByRole("button", { name: "Wrap", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    const sidebar = page.getByRole("complementary", {
      name: "Repository details",
      exact: true,
    });
    await sidebar.getByRole("button", { name: "Edit website links" }).click();
    await sidebar.getByLabel("Name", { exact: true }).fill("Hosted demo");
    await sidebar
      .getByLabel("URL", { exact: true })
      .fill("https://example.com/demo");
    await sidebar.getByRole("button", { name: "Save", exact: true }).click();
    await expect(
      sidebar.getByRole("link", { name: "Hosted demo" }),
    ).toHaveAttribute("href", "https://example.com/demo");
    await page.getByRole("button", { name: "History", exact: true }).click();
    await expect(
      page.getByRole("heading", { name: "Commit history" }),
    ).toBeVisible();
    await expect(
      page.getByText("Update greeting", { exact: true }),
    ).toBeVisible();
    await expect(page.getByText("Add README", { exact: true })).toHaveCount(0);
    for (const width of [1280, 390]) {
      await page.setViewportSize({ width, height: 900 });
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        ),
      ).toBe(true);
      const bounds = await sidebar
        .locator(":scope > section")
        .first()
        .evaluate((toolbar) => {
          const rect = toolbar.getBoundingClientRect();
          return [...toolbar.children].every((child) => {
            const box = child.getBoundingClientRect();
            return (
              box.width === 0 ||
              (box.left >= rect.left && box.right <= rect.right)
            );
          });
        });
      expect(bounds).toBe(true);
    }
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.goto(`/${repository}?path=README.md`);
    await expect(
      page.getByRole("heading", { name: "Demo", exact: true }),
    ).toBeVisible();
    await expect(page.getByRole("table")).toBeVisible();
    await page.getByRole("radio", { name: "Source", exact: true }).click();
    await expect(
      page.getByRole("button", { name: "Select line 1", exact: true }),
    ).toBeVisible();
    await page.goto(`/${repository}?view=actions`);
    await expect(
      page
        .getByRole("list", { name: "Workflow folders in search order" })
        .getByRole("listitem"),
    ).toHaveCount(3);
    expect(errors).toEqual([]);
  } finally {
    await page.request.post(`${api}/delete`, { timeout: 5000 }).catch(() => undefined);
    await page.request.delete(`${api}/purge`, { timeout: 5000 }).catch(() => undefined);
  }
});
