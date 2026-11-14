import { expect, test } from "@playwright/test";

test("live syntax highlighting preserves native file editing", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const username = process.env.GITADEL_E2E_USERNAME ?? "e2e-admin";
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/login");
  await page.getByLabel("Username").fill(username);
  await page
    .getByLabel("Password", { exact: true })
    .fill(process.env.GITADEL_E2E_PASSWORD ?? "e2e-password-123456");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).not.toHaveURL(/\/login(?:\?|$)/);
  const name = `file-editor-${Date.now()}`;
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
      'export const greeting = "hello";\n// <script>window.editorInjected = true</script>\n' +
      `// ${"long line ".repeat(100)}\n` +
      Array.from({ length: 80 }, (_, i) => `// line ${i + 4}\n`).join("");
    expect(
      (
        await page.request.post(`${api}/files`, {
          data: {
            branch: "main",
            expected_commit: null,
            path: "demo.mjs",
            content: source,
            message: "Add editor fixture",
          },
        })
      ).status(),
    ).toBe(200);
    await page.goto(`/${repository}?path=demo.mjs`);
    await page.getByRole("button", { name: "Edit file", exact: true }).click();
    const editor = page.getByRole("textbox", {
      name: "Contents of demo.mjs",
      exact: true,
    });
    const layer = page.locator("[data-editor-highlight]");
    expect((await layer.boundingBox())!.height).toBe((await editor.boundingBox())!.height);
    expect((await layer.boundingBox())!.height).toBeGreaterThan(1600);
    await expect(editor).toHaveValue(source);
    await expect(layer.locator(".hljs-keyword").first()).toHaveText("export");
    await expect(layer.locator(".hljs-string")).toHaveText('"hello"');
    expect(await layer.textContent()).toBe(source + "\n");
    expect(await page.evaluate(() => "editorInjected" in window)).toBe(false);
    // The mirror must use precisely the textarea's character and line metrics.
    const metrics = await editor.evaluate((textarea) => {
      const mirror = textarea.previousElementSibling!;
      const input = getComputedStyle(textarea);
      const output = getComputedStyle(mirror);
      return [
        "fontFamily",
        "fontSize",
        "lineHeight",
        "letterSpacing",
        "tabSize",
        "paddingLeft",
        "paddingTop",
      ].map((property) => [
        input[property as keyof CSSStyleDeclaration],
        output[property as keyof CSSStyleDeclaration],
      ]);
    });
    for (const [input, output] of metrics) expect(input).toBe(output);
    await editor.press("Control+End");
    await editor.pressSequentially('export const added = "updated";');
    await expect(layer.locator(".hljs-string").last()).toHaveText('"updated"');
    await editor.press("Control+z");
    await expect(editor).not.toHaveValue(
      source + 'export const added = "updated";',
    );
    // Horizontal scrolling follows the native textarea, including caret scrolling.
    await editor.evaluate((input: HTMLTextAreaElement) => {
      input.scrollLeft = 240;
      input.dispatchEvent(new Event("scroll"));
    });
    expect(await layer.evaluate((node) => node.scrollLeft)).toBeGreaterThan(0);
    expect(await layer.evaluate((node) => node.scrollLeft)).toBe(
      await editor.evaluate((node: HTMLTextAreaElement) => node.scrollLeft),
    );
    const updated = 'export const greeting = "saved";\n\treturn greeting;\n';
    await editor.fill(updated);
    await expect(layer.locator(".hljs-string")).toHaveText('"saved"');
    await editor.press("Control+Home");
    await editor.press("Tab");
    await expect(editor).toHaveValue("  " + updated);
    await editor.press("Shift+Tab");
    await expect(editor).toHaveValue(updated);
    await editor.press("Control+s");
    await expect(page.getByRole("dialog")).toBeVisible();
    await page.getByRole("button", { name: "Commit", exact: true }).click();
    await expect(editor).toHaveCount(0);
    await expect(page.locator('[data-code-line="1"] .hljs-string')).toHaveText(
      '"saved"',
    );
    const blob = await page.request.get(`${api}/blob?rev=main&path=demo.mjs`);
    expect((await blob.json()).content).toBe(updated);
    await page.getByRole("button", { name: "Edit file", exact: true }).click();
    // Oversized drafts stay visible and editable rather than freezing the editor.
    await editor.fill("x".repeat(200_001));
    await expect(editor).not.toHaveClass(/highlighted/);
    expect(
      await editor.evaluate((node) => getComputedStyle(node).color),
    ).not.toBe("rgba(0, 0, 0, 0)");
    await editor.fill('export const greeting = "small again";\n');
    await expect(layer.locator(".hljs-string")).toHaveText('"small again"');
    await expect(editor).toHaveClass(/highlighted/);
    await page.emulateMedia({ forcedColors: "active" });
    await expect(layer).toHaveCSS("visibility", "hidden");
    expect(
      await editor.evaluate((node) => getComputedStyle(node).color),
    ).not.toBe("rgba(0, 0, 0, 0)");
    await editor.fill(updated);
    await page.getByRole("button", { name: "Cancel", exact: true }).click();
    expect(errors).toEqual([]);
  } finally {
    await page.request.post(`${api}/delete`);
    await page.request.delete(`${api}/purge`);
  }
});
