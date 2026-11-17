import { createHash } from 'node:crypto';
import { readdir, readFile, writeFile, stat } from 'node:fs/promises';
import { resolve, relative } from 'node:path';

const root = resolve(import.meta.dir, '../..');
async function run(command: string[]) {
  const child = Bun.spawn(command, { cwd: root, stdout: 'inherit', stderr: 'inherit', env: process.env });
  if (await child.exited !== 0) process.exit(child.exitCode ?? 1);
}
await run(['bash', 'scripts/api-contract.sh', 'check']);
await run(['bun', 'run', '--cwd', 'frontend', 'check']);
await run(['bun', 'run', '--cwd', 'frontend', 'build:assets']);

const files: string[] = [];
async function collect(path: string) {
  const entries = await readdir(resolve(root, path), { withFileTypes: true });
  for (const entry of entries) {
    const child = `${path}/${entry.name}`;
    if (entry.isDirectory()) await collect(child);
    else if (entry.isFile()) files.push(child);
    else throw new Error(`Contract inputs must be regular files: ${child}`);
  }
}
for (const input of (await readFile(resolve(root, 'api/inputs.txt'), 'utf8')).trim().split('\n')) {
  const path = resolve(root, input);
  const info = await stat(path);
  if (info.isDirectory()) await collect(input);
  else files.push(relative(root, path));
}
const digest = createHash('sha256');
for (const file of files.sort((left, right) => Buffer.compare(Buffer.from(left), Buffer.from(right)))) {
  const bytes = await readFile(resolve(root, file));
  digest.update(`${file}\0${bytes.length}\0`);
  digest.update(bytes);
}
await writeFile(resolve(root, 'frontend/build/.api-contract-checked'), `${digest.digest('hex')}\n`);
