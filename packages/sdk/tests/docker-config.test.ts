import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { dockerConfigAuth } from '../src/docker-config.node.ts';

let dir: string;

beforeEach(async () => {
  dir = await mkdtemp(join(tmpdir(), 'akua-docker-cfg-'));
});

afterEach(async () => {
  await rm(dir, { recursive: true, force: true });
});

async function writeConfig(contents: unknown): Promise<string> {
  const path = join(dir, 'config.json');
  await writeFile(path, JSON.stringify(contents));
  return path;
}

describe('dockerConfigAuth', () => {
  it('decodes base64 `auth` entries into username/password', async () => {
    const auth = Buffer.from('alice:s3cret', 'utf8').toString('base64');
    const path = await writeConfig({
      auths: { 'ghcr.io': { auth } },
    });
    expect(await dockerConfigAuth({ path })).toEqual({
      'ghcr.io': { username: 'alice', password: 's3cret' },
    });
  });

  it('prefers identitytoken over basic auth', async () => {
    const path = await writeConfig({
      auths: {
        'ghcr.io': { identitytoken: 'ghp_abc', username: 'ignored', password: 'ignored' },
      },
    });
    expect(await dockerConfigAuth({ path })).toEqual({
      'ghcr.io': { token: 'ghp_abc' },
    });
  });

  it('accepts literal username/password fields when no `auth`', async () => {
    const path = await writeConfig({
      auths: { 'example.com': { username: 'bob', password: 'pw' } },
    });
    expect(await dockerConfigAuth({ path })).toEqual({
      'example.com': { username: 'bob', password: 'pw' },
    });
  });

  it('normalises https://.../v1/ server addresses to bare host', async () => {
    const auth = Buffer.from('u:p', 'utf8').toString('base64');
    const path = await writeConfig({
      auths: { 'https://index.docker.io/v1/': { auth } },
    });
    const out = await dockerConfigAuth({ path });
    expect(out['index.docker.io']).toEqual({ username: 'u', password: 'p' });
  });

  it('returns {} when config file is missing (no ENOENT propagation)', async () => {
    expect(await dockerConfigAuth({ path: join(dir, 'nope.json') })).toEqual({});
  });

  it('filters to requested hosts only', async () => {
    const a = Buffer.from('u:p', 'utf8').toString('base64');
    const path = await writeConfig({
      auths: {
        'ghcr.io': { auth: a },
        'example.com': { auth: a },
      },
    });
    const out = await dockerConfigAuth({ path, hosts: ['ghcr.io'] });
    expect(out).toEqual({ 'ghcr.io': { username: 'u', password: 'p' } });
  });

  it('skips cred helpers that error (binary missing)', async () => {
    const path = await writeConfig({
      credHelpers: { 'nonexistent.example.com': 'definitely-not-a-binary' },
    });
    // Should not throw; just return no entry for that host.
    const out = await dockerConfigAuth({ path });
    expect(out['nonexistent.example.com']).toBeUndefined();
  });
});
