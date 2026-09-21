import { expect, test } from "@playwright/test";

const username = process.env.GITADEL_E2E_USERNAME ?? "e2e-admin";
const password = process.env.GITADEL_E2E_PASSWORD ?? "e2e-password-123456";

test("private attachment access follows authentication and deletion", async ({
  browser,
  page,
}) => {
  await page.goto("/login");
  await expect(page.getByRole("heading", { name: /sign in/i })).toBeVisible();
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page).not.toHaveURL(/\/login(?:\?|$)/);

  const repositoryName = `security-${Date.now()}`;
  const created = await page.request.post("/api/v1/repositories", {
    data: {
      namespace: username,
      name: repositoryName,
      description: null,
      visibility: "private",
      object_format: "sha1",
    },
  });
  expect(created.status()).toBe(201);

  try {
    const uploaded = await page.request.put(
      `/api/v1/repositories/${username}/${repositoryName}/issue-attachments?name=proof.txt`,
      {
        data: "private attachment",
        headers: { "content-type": "text/plain" },
      },
    );
    expect(uploaded.status()).toBe(201);
    const attachment = (await uploaded.json()) as { id: string; url: string };

    const anonymous = await browser.newContext();
    const anonymousDownload = await anonymous.request.get(attachment.url);
    expect(anonymousDownload.status()).toBe(404);
    await anonymous.close();

    const download = await page.request.get(attachment.url);
    expect(download.status()).toBe(200);
    expect(download.headers()["x-content-type-options"]).toBe("nosniff");
    expect(download.headers()["content-security-policy"]).toBe(
      "default-src 'none'; sandbox",
    );
    expect(download.headers()["content-disposition"]).toContain("attachment");

    const deleted = await page.request.delete(
      `/api/v1/repositories/${username}/${repositoryName}/issue-attachments/${attachment.id}`,
    );
    expect(deleted.status()).toBe(204);
    expect((await page.request.get(attachment.url)).status()).toBe(404);
  } finally {
    await page.request.post(
      `/api/v1/repositories/${username}/${repositoryName}/delete`,
    );
    await page.request.delete(
      `/api/v1/repositories/${username}/${repositoryName}/purge`,
    );
  }
});
