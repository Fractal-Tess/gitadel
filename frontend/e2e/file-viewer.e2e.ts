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
            content:
              "# Demo\n\nRead me. See [setup](#getting-started).\n\n| Feature | Status |\n| --- | --- |\n| Tables | Rendered |\n\n## Getting started\n\nInstall it.\n",
            message: "Add README",
          },
        })
      ).status(),
    ).toBe(200);
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.goto(`/${repository}?path=src%2Fdemo.ts`);
    await expect(page.getByText("1,105 lines", { exact: true })).toBeVisible();
    await expect(page.locator("[data-code-line]")).toHaveCount(1000);
    // Reading controls share the file's own header row instead of stacking.
    const header = page.locator("header", { has: page.getByRole("group", { name: "File view" }) });
    expect((await header.boundingBox())!.height).toBeLessThan(56);
    for (const label of ["Find in file", "Outline", "Display settings", "Copy permanent link", "More file actions"])
      await expect(page.getByRole("button", { name: label, exact: true })).toBeVisible();
    // Large files render a bounded window with inline paging.
    await page.getByRole("button", { name: "Show lines 1,001–1,105" }).click();
    await expect(page.locator('[data-code-line="1105"]')).toBeVisible();
    await expect(page.locator("[data-code-line]")).toHaveCount(105);
    await page.getByRole("button", { name: "Show lines 1–1,000" }).click();
    await expect(page.locator('[data-code-line="1000"]')).toBeVisible();
    await page
      .getByRole("button", { name: "Select line 2", exact: true })
      .click();
    await page
      .getByRole("button", { name: "Select line 3", exact: true })
      .click({ modifiers: ["Shift"] });
    await expect(page).toHaveURL(/#L2-L3$/);
    const selection = page.getByRole("toolbar", { name: "Lines 2–3 selected" });
    await expect(selection).toBeVisible();
    // Arrow keys move between line numbers; Shift extends the selection.
    await page.getByRole("button", { name: "Select line 3", exact: true }).press("Shift+ArrowDown");
    await expect(page).toHaveURL(/#L2-L4$/);
    await expect(page.getByRole("button", { name: "Select line 4", exact: true })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(selection).toHaveCount(0);
    await expect(page).not.toHaveURL(/#L/);
    // Dragging down the gutter selects a range.
    await page.getByRole("button", { name: "Select line 2", exact: true }).hover();
    await page.mouse.down();
    await page.getByRole("button", { name: "Select line 3", exact: true }).hover();
    await page.mouse.up();
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
    await page.getByRole("button", { name: "More file actions" }).click();
    await page.getByRole("menuitem", { name: /Go to line/ }).click();
    await page.getByLabel("Line or range", { exact: true }).fill("2000");
    await page.getByRole("button", { name: "Go", exact: true }).click();
    await expect(page.getByText("Enter a line or range from 1 to 1,105.")).toBeVisible();
    await page.getByLabel("Line or range", { exact: true }).fill("1100");
    await page.getByRole("button", { name: "Go", exact: true }).click();
    await expect(
      page.getByRole("button", { name: "Select line 1100", exact: true }),
    ).toBeVisible();
    await expect(page.getByLabel("Line or range", { exact: true })).toHaveCount(0);
    // The find bar floats over the code rather than pushing it down.
    await page.getByRole("button", { name: "Select line 1100", exact: true }).click();
    const before = (await page.locator('[data-code-line="1100"]').boundingBox())!.y;
    await page.keyboard.press("Control+f");
    const find = page.getByRole("textbox", { name: "Find in file", exact: true });
    await expect(find).toBeFocused();
    expect((await page.locator('[data-code-line="1100"]').boundingBox())!.y).toBe(before);
    await find.fill("updated");
    await expect(page.getByText("1 of 1", { exact: true })).toBeVisible();
    await expect(page.locator('[data-code-line="2"] mark')).toHaveText(
      "updated",
    );
    // Marks sit inside the syntax colouring instead of replacing it.
    await expect(page.locator('[data-code-line="2"] [class^="hljs-"] mark')).toHaveCount(1);
    await find.press("Escape");
    await expect(find).toHaveCount(0);
    await expect(page.locator('[data-code-line="2"] mark')).toHaveCount(0);
    await page.getByRole("button", { name: "Display settings" }).click();
    await page.getByRole("switch", { name: "Wrap long lines" }).click();
    await page.getByRole("button", { name: "Larger text" }).click();
    await page.keyboard.press("Escape");
    await page.reload();
    await page.getByRole("button", { name: "Display settings" }).click();
    await expect(page.getByRole("switch", { name: "Wrap long lines" })).toBeChecked();
    await expect(page.getByText("13px", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Reset to defaults" }).click();
    await expect(page.getByRole("switch", { name: "Wrap long lines" })).not.toBeChecked();
    await page.keyboard.press("Escape");
    const sidebar = page.getByRole("complementary", {
      name: "Repository details",
      exact: true,
    });
    await sidebar.getByRole("button", { name: "Add a website" }).click();
    await expect(sidebar.getByLabel("Website 1 URL")).toBeFocused();
    await sidebar.getByLabel("Website 1 URL").fill("ftp://example.com");
    await sidebar.getByRole("button", { name: "Save", exact: true }).click();
    await expect(sidebar.getByText("Use an http:// or https:// address.")).toBeVisible();
    // A bare host gets https:// rather than an error.
    await sidebar.getByLabel("Website 1 URL").fill("example.com/demo");
    await sidebar.getByLabel("Website 1 name").fill("Hosted demo");
    await sidebar.getByRole("button", { name: "Add link" }).click();
    await sidebar.getByLabel("Website 2 URL").fill("https://docs.example.com/");
    await sidebar.getByRole("button", { name: "Move website 2 up" }).click();
    await expect(sidebar.getByLabel("Website 1 URL")).toHaveValue("https://docs.example.com/");
    await sidebar.getByRole("button", { name: "Save", exact: true }).click();
    const links = sidebar
      .locator("section", { has: page.getByRole("heading", { name: "Websites" }) })
      .getByRole("link");
    await expect(
      sidebar.getByRole("link", { name: /Hosted demo/ }),
    ).toHaveAttribute("href", "https://example.com/demo");
    // An unnamed link is labelled by its host, which the server fills in.
    await expect(sidebar.getByRole("link", { name: /^docs\.example\.com/ })).toHaveAttribute(
      "href",
      "https://docs.example.com/",
    );
    // Saved in the order they were arranged.
    expect(
      await links.evaluateAll((items) => items.map((item) => item.getAttribute("href"))),
    ).toEqual(["https://docs.example.com/", "https://example.com/demo"]);
    await page.getByRole("button", { name: "More file actions" }).click();
    await page.getByRole("menuitem", { name: "File history" }).click();
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
    // Headings get shareable anchors, and a heading hash keeps the preview.
    await page.getByRole("link", { name: "setup", exact: true }).click();
    await expect(page).toHaveURL(/#user-content-getting-started$/);
    await page.reload();
    await expect(page.getByRole("heading", { name: /Getting started/ })).toBeInViewport();
    await expect(page.getByRole("link", { name: "Getting started", exact: true })).toHaveAttribute(
      "href",
      "#user-content-getting-started",
    );
    await expect(page.getByRole("radio", { name: "Preview", exact: true })).toBeChecked();
    await page.getByRole("radio", { name: "Source", exact: true }).click();
    await expect(
      page.getByRole("button", { name: "Select line 1", exact: true }),
    ).toBeVisible();
    await page.goto(`/${repository}?view=actions`);
    await expect(page.getByText("Set up a workflow", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Copy starter workflow" }).click();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toContain(
      "runs-on: docker",
    );
    await expect(
      page
        .getByRole("list", { name: "Workflow folders in search order" })
        .getByRole("listitem"),
    ).toHaveCount(3);
    const folderRows = page.getByRole("list", { name: "Workflow folders in search order" }).getByRole("listitem");
    const tops = await folderRows.evaluateAll((items) => items.map((item) => item.getBoundingClientRect().top));
    expect(tops[0]).toBeLessThan(tops[1]);
    expect(tops[1]).toBeLessThan(tops[2]);
    expect(errors).toEqual([]);
  } finally {
    await page.request.post(`${api}/delete`, { timeout: 5000 }).catch(() => undefined);
    await page.request.delete(`${api}/purge`, { timeout: 5000 }).catch(() => undefined);
  }
});
