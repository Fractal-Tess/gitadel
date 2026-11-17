import { readFile } from 'node:fs/promises';

const document = JSON.parse(await readFile(process.argv[2] ?? new URL('../../api/openapi.json', import.meta.url), 'utf8'));
function resolve(reference: string): unknown {
  if (!reference.startsWith('#/')) throw new Error(`Offline contracts require local references: ${reference}`);
  let value = document;
  for (const part of reference.slice(2).split('/')) value = value?.[part.replaceAll('~1', '/').replaceAll('~0', '~')];
  if (value === undefined) throw new Error(`Unresolved OpenAPI reference: ${reference}`);
  return value;
}
function visit(value: unknown): void {
  if (!value || typeof value !== 'object') return;
  if ('$ref' in value && typeof value.$ref === 'string') resolve(value.$ref);
  for (const item of Object.values(value)) visit(item);
}
visit(document);

function typed(schema: any, seen = new Set<string>()): boolean {
  if (!schema || typeof schema !== 'object') return false;
  if (schema.$ref) {
    if (seen.has(schema.$ref)) return true;
    return typed(resolve(schema.$ref), new Set([...seen, schema.$ref]));
  }
  if (schema.oneOf || schema.anyOf || schema.allOf) {
    const variants = schema.oneOf ?? schema.anyOf ?? schema.allOf;
    return variants.length > 0 && variants.every((s: unknown) => typed(s, seen));
  }
  if (schema.type === 'array') return typed(schema.items, seen);
  if (schema.type === 'object') return Object.keys(schema.properties ?? {}).length > 0 || typed(schema.additionalProperties, seen);
  return ['string', 'integer', 'number', 'boolean', 'null'].includes(schema.type);
}
for (const [path, item] of Object.entries(document.paths) as [string, Record<string, any>][]) {
  for (const [method, operation] of Object.entries(item)) {
    if (!['get', 'post', 'put', 'patch', 'delete'].includes(method)) continue;
    const request = operation.requestBody?.content?.['application/json'];
    if (request && !typed(request.schema)) throw new Error(`${method} ${path}: JSON request has no concrete DTO`);
    for (const [status, response] of Object.entries(operation.responses) as [string, any][]) {
      const json = response.content?.['application/json'];
      if (json && !typed(json.schema)) throw new Error(`${method} ${path} (${status}): JSON response has no concrete DTO`);
    }
  }
}
