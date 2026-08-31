import { toast } from "svelte-sonner";

import {
  memberSchema,
  organizationSchema,
  type Member,
  type Organization,
} from "$lib/api/organizations.js";
import {
  ApiFailure,
  jsonBody,
  requestEmpty,
  requestJson,
} from "$lib/api/transport.js";
import type { AuthorizationCacheScope } from "$lib/cache-scope.js";
import { takeNamespaceMembers } from "$lib/namespace-preload.js";
import type { AppState } from "$lib/state/app-state.svelte.js";

type ScopeGuard = (scope: AuthorizationCacheScope) => boolean;
type Changed = (organizations: Organization[]) => void;

export class OrganizationSettingsState {
  organizations = $state.raw<Organization[]>([]);
  selectedOrganization = $state.raw<Organization | null>(null);
  members = $state.raw<Member[]>([]);
  organizationSlug = $state("");
  organizationDisplayName = $state("");
  memberUsername = $state("");
  memberRole = $state<"owner" | "member">("member");
  working = $state(false);
  membersLoading = $state(false);
  membersLoadError = $state<string | null>(null);

  private scope: AuthorizationCacheScope;
  constructor(
    private readonly app: AppState,
    initial: Organization[],
    scope: AuthorizationCacheScope,
    private readonly current: ScopeGuard,
    private readonly changed: Changed,
  ) {
    this.scope = scope;
    this.organizations = initial;
  }

  setScope(scope: AuthorizationCacheScope): void {
    this.scope = scope;
  }

  async createOrganization(): Promise<boolean> {
    const scope = this.scope;
    return this.run(scope, async () => {
      const organization = await requestJson(
        "/api/v1/organizations",
        organizationSchema,
        {
          method: "POST",
          body: jsonBody({
            slug: this.organizationSlug,
            display_name: this.organizationDisplayName,
          }),
        },
      );
      if (!this.current(scope)) return;
      this.organizations = [...this.organizations, organization];
      this.app.addOrganization(organization);
      this.changed(this.organizations);
      this.organizationSlug = "";
      this.organizationDisplayName = "";
      await this.selectOrganization(organization);
      toast.success("Organization created");
    });
  }

  async selectOrganization(organization: Organization): Promise<void> {
    const scope = this.scope;
    this.selectedOrganization = organization;
    this.membersLoading = true;
    this.membersLoadError = null;
    try {
      const members = await takeNamespaceMembers(organization.slug, scope);
      if (!this.current(scope)) return;
      this.members = members;
    } catch (caught) {
      if (!this.current(scope)) return;
      this.membersLoadError =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "Could not load organization members.";
      throw caught;
    } finally {
      if (this.current(scope)) this.membersLoading = false;
    }
  }

  async addMember(): Promise<boolean> {
    const scope = this.scope;
    const organization = this.selectedOrganization;
    if (!organization) return false;
    return this.run(scope, async () => {
      const member = await requestJson(
        `/api/v1/organizations/${organization.slug}/members`,
        memberSchema,
        {
          method: "POST",
          body: jsonBody({
            username: this.memberUsername,
            role: this.memberRole,
          }),
        },
      );
      if (!this.current(scope)) return;
      this.members = [...this.members, member];
      this.memberUsername = "";
      toast.success("Organization member added");
    });
  }

  async removeMember(username: string): Promise<boolean> {
    const scope = this.scope;
    const organization = this.selectedOrganization;
    if (!organization) return false;
    return this.run(scope, async () => {
      await requestEmpty(
        `/api/v1/organizations/${organization.slug}/members/${username}`,
        { method: "DELETE" },
      );
      if (!this.current(scope)) return;
      this.members = this.members.filter(
        (member) => member.username !== username,
      );
      toast.success("Organization member removed");
    });
  }

  private async run(
    scope: AuthorizationCacheScope,
    task: () => Promise<void>,
  ): Promise<boolean> {
    this.working = true;
    try {
      await task();
      return true;
    } catch (caught) {
      if (!this.current(scope)) return false;
      const message =
        caught instanceof ApiFailure || caught instanceof Error
          ? caught.message
          : "The request failed.";
      toast.error(message);
      return false;
    } finally {
      this.working = false;
    }
  }
}
