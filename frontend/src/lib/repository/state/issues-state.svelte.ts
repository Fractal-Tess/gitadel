import { z } from "zod";
import { toast } from "svelte-sonner";
import { renderedMarkdownSchema } from "$lib/api/repositories.js";
import {
  issueAttachmentSchema,
  issueCommentSchema,
  issueLabelSchema,
  issueSchema,
  type Issue,
  type IssueAttachment,
  type IssueComment,
  type IssueLabel,
  type IssueUser,
} from "$lib/api/issues.js";
import { jsonBody, requestEmpty, requestJson } from "$lib/api/transport.js";
import {
  loadRepositoryAssignableUsers,
  loadRepositoryIssueLabels,
  loadRepositoryIssues,
} from "$lib/repository/repository-data-cache.js";
import type { RepositoryFeatureContext } from "./shared.js";
import { errorMessage, repositoryApi } from "./shared.js";

type IssuesCallbacks = {
  setError(message: string): void;
  invalidateData(datasets?: readonly string[]): void;
  selectIssue(number: number | null): void;
  getIssueNumber(): number | null;
  isScopeCurrent(): boolean;
};

export class RepositoryIssuesState {
  readonly namespace: string;
  readonly name: string;
  readonly scope: RepositoryFeatureContext["scope"];
  readonly setError: (message: string) => void;
  readonly invalidateData: IssuesCallbacks["invalidateData"];
  readonly selectIssue: IssuesCallbacks["selectIssue"];
  readonly getIssueNumber: IssuesCallbacks["getIssueNumber"];
  readonly isScopeCurrent: IssuesCallbacks["isScopeCurrent"];
  issues = $state.raw<Issue[]>([]);
  selectedIssue = $state.raw<Issue | null>(null);
  issueComments = $state.raw<IssueComment[]>([]);
  issueLabels = $state.raw<IssueLabel[]>([]);
  assignableUsers = $state.raw<IssueUser[]>([]);
  issuesLoading = $state(false);
  issuesLoaded = $state(false);
  issuePending = $state(false);
  commentPending = $state(false);
  labelPending = $state(false);

  #destroyed = false;
  constructor(context: RepositoryFeatureContext, callbacks: IssuesCallbacks) {
    this.namespace = context.locator.namespace;
    this.name = context.locator.name;
    this.scope = context.scope;
    this.setError = callbacks.setError;
    this.invalidateData = callbacks.invalidateData;
    this.selectIssue = callbacks.selectIssue;
    this.getIssueNumber = callbacks.getIssueNumber;
    this.isScopeCurrent = callbacks.isScopeCurrent;
  }
  destroy(): void {
    this.#destroyed = true;
  }

  async loadIssues(init: RequestInit = {}): Promise<void> {
    this.issuesLoading = true;
    try {
      const issues = await loadRepositoryIssues(
        this.namespace,
        this.name,
        this.scope,
        !init.signal,
      );
      if (init.signal?.aborted || this.#destroyed || !this.isScopeCurrent())
        return;
      this.issues = issues;
      this.issuesLoaded = true;
    } finally {
      this.issuesLoading = false;
    }
  }
  async loadIssue(number: number, init: RequestInit = {}): Promise<Issue> {
    const [issue, comments] = await Promise.all([
      requestJson(repositoryApi(this, `/issues/${number}`), issueSchema, init),
      requestJson(
        repositoryApi(this, `/issues/${number}/comments`),
        z.array(issueCommentSchema),
        init,
      ),
    ]);
    if (
      !this.#destroyed &&
      this.getIssueNumber() === number &&
      this.isScopeCurrent()
    ) {
      this.selectedIssue = issue;
      this.issueComments = comments;
    }
    return issue;
  }
  async loadIssueLabels(init: RequestInit = {}): Promise<void> {
    const labels = await loadRepositoryIssueLabels(
      this.namespace,
      this.name,
      this.scope,
    );
    if (!init.signal?.aborted && !this.#destroyed && this.isScopeCurrent())
      this.issueLabels = labels;
  }
  async loadAssignableUsers(init: RequestInit = {}): Promise<void> {
    const users = await loadRepositoryAssignableUsers(
      this.namespace,
      this.name,
      this.scope,
    );
    if (!init.signal?.aborted && !this.#destroyed && this.isScopeCurrent())
      this.assignableUsers = users;
  }
  async createIssue(values: {
    title: string;
    body: string;
    label_ids: string[];
    assignee?: string;
    attachment_ids?: string[];
  }): Promise<Issue> {
    this.issuePending = true;
    try {
      const issue = await requestJson(
        repositoryApi(this, "/issues"),
        issueSchema,
        { method: "POST", body: jsonBody(values) },
      );
      if (!this.isScopeCurrent()) return issue;
      toast.success(`Issue #${issue.number} opened.`);
      await this.loadIssues();
      this.selectIssue(issue.number);
      return issue;
    } catch (caught) {
      this.setError(errorMessage(caught));
      throw caught;
    } finally {
      this.issuePending = false;
    }
  }
  async updateIssue(
    number: number,
    values: {
      title?: string;
      body?: string;
      state?: "open" | "closed";
      assignee?: string;
      label_ids?: string[];
      attachment_ids?: string[];
    },
  ): Promise<Issue> {
    this.issuePending = true;
    try {
      const issue = await requestJson(
        repositoryApi(this, `/issues/${number}`),
        issueSchema,
        { method: "PATCH", body: jsonBody(values) },
      );
      if (!this.isScopeCurrent()) return issue;
      this.invalidateData(["issues"]);
      this.selectedIssue = issue;
      this.issues = this.issues.map((item) =>
        item.number === issue.number ? issue : item,
      );
      toast.success(`Issue #${number} updated.`);
      return issue;
    } catch (caught) {
      this.setError(errorMessage(caught));
      throw caught;
    } finally {
      this.issuePending = false;
    }
  }
  async deleteIssue(number: number): Promise<void> {
    this.issuePending = true;
    try {
      await requestEmpty(repositoryApi(this, `/issues/${number}`), {
        method: "DELETE",
      });
      if (!this.isScopeCurrent()) return;
      toast.success(`Issue #${number} deleted.`);
      this.selectedIssue = null;
      this.issueComments = [];
      await this.loadIssues();
      this.selectIssue(null);
    } catch (caught) {
      this.setError(errorMessage(caught));
      throw caught;
    } finally {
      this.issuePending = false;
    }
  }
  async createIssueComment(
    number: number,
    body: string,
  ): Promise<IssueComment> {
    this.commentPending = true;
    try {
      const comment = await requestJson(
        repositoryApi(this, `/issues/${number}/comments`),
        issueCommentSchema,
        { method: "POST", body: jsonBody({ body }) },
      );
      if (!this.isScopeCurrent()) return comment;
      this.issueComments = [...this.issueComments, comment];
      if (this.selectedIssue?.number === number)
        this.selectedIssue = {
          ...this.selectedIssue,
          comment_count: this.selectedIssue.comment_count + 1,
        };
      await this.loadIssues();
      return comment;
    } catch (caught) {
      this.setError(errorMessage(caught));
      throw caught;
    } finally {
      this.commentPending = false;
    }
  }
  async updateIssueComment(
    number: number,
    id: string,
    body: string,
  ): Promise<IssueComment> {
    this.commentPending = true;
    try {
      const comment = await requestJson(
        repositoryApi(this, `/issues/${number}/comments/${id}`),
        issueCommentSchema,
        { method: "PATCH", body: jsonBody({ body }) },
      );
      if (!this.isScopeCurrent()) return comment;
      this.issueComments = this.issueComments.map((item) =>
        item.id === id ? comment : item,
      );
      return comment;
    } catch (caught) {
      this.setError(errorMessage(caught));
      throw caught;
    } finally {
      this.commentPending = false;
    }
  }
  async deleteIssueComment(number: number, id: string): Promise<void> {
    this.commentPending = true;
    try {
      await requestEmpty(
        repositoryApi(this, `/issues/${number}/comments/${id}`),
        { method: "DELETE" },
      );
      if (!this.isScopeCurrent()) return;
      this.issueComments = this.issueComments.filter((item) => item.id !== id);
      if (this.selectedIssue?.number === number)
        this.selectedIssue = {
          ...this.selectedIssue,
          comment_count: Math.max(0, this.selectedIssue.comment_count - 1),
        };
      await this.loadIssues();
    } catch (caught) {
      this.setError(errorMessage(caught));
      throw caught;
    } finally {
      this.commentPending = false;
    }
  }
  async createIssueLabel(values: {
    name: string;
    color: string;
    description: string;
  }): Promise<IssueLabel> {
    this.labelPending = true;
    try {
      const label = await requestJson(
        repositoryApi(this, "/issue-labels"),
        issueLabelSchema,
        { method: "POST", body: jsonBody(values) },
      );
      if (!this.isScopeCurrent()) return label;
      this.invalidateData(["issue-labels"]);
      this.issueLabels = [...this.issueLabels, label].sort((a, b) =>
        a.name.localeCompare(b.name),
      );
      return label;
    } catch (caught) {
      this.setError(errorMessage(caught));
      throw caught;
    } finally {
      this.labelPending = false;
    }
  }
  async deleteIssueLabel(id: string): Promise<void> {
    this.labelPending = true;
    try {
      await requestEmpty(repositoryApi(this, `/issue-labels/${id}`), {
        method: "DELETE",
      });
      if (!this.isScopeCurrent()) return;
      this.invalidateData(["issue-labels"]);
      this.issueLabels = this.issueLabels.filter((label) => label.id !== id);
      await this.loadIssues();
      const number = this.getIssueNumber();
      if (number) await this.loadIssue(number);
    } catch (caught) {
      this.setError(errorMessage(caught));
      throw caught;
    } finally {
      this.labelPending = false;
    }
  }
  async uploadIssueAttachment(file: File): Promise<IssueAttachment> {
    return requestJson(
      `${repositoryApi(this, "/issue-attachments")}?${new URLSearchParams({ name: file.name })}`,
      issueAttachmentSchema,
      {
        method: "PUT",
        headers: { "content-type": file.type || "application/octet-stream" },
        body: file,
      },
    );
  }
  async previewMarkdown(markdown: string): Promise<string> {
    const response = await requestJson(
      repositoryApi(this, "/markdown-preview"),
      renderedMarkdownSchema,
      {
        method: "POST",
        body: jsonBody({ markdown }),
      },
    );
    return response.rendered_html;
  }
}
