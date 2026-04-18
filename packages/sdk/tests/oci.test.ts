import { describe, it, expect } from 'vitest';

import { parseOciRef, parseBearerChallenge, OciPullError } from '../src/oci.ts';

describe('parseOciRef', () => {
  it('splits host / repository / tag for a nested path', () => {
    expect(parseOciRef('oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1')).toEqual({
      host: 'ghcr.io',
      repository: 'stefanprodan/charts/podinfo',
      tag: '6.7.1',
    });
  });

  it('splits host / repository / tag for a single-segment path', () => {
    expect(parseOciRef('oci://registry.local/mychart:1.0.0')).toEqual({
      host: 'registry.local',
      repository: 'mychart',
      tag: '1.0.0',
    });
  });

  it('rejects a missing scheme', () => {
    expect(() => parseOciRef('ghcr.io/foo/bar:1.0')).toThrow(OciPullError);
  });

  it('rejects a missing tag', () => {
    expect(() => parseOciRef('oci://ghcr.io/foo/bar')).toThrow(/missing :<version>/);
  });

  it('rejects a missing chart path', () => {
    expect(() => parseOciRef('oci://ghcr.io:1.0.0')).toThrow(/missing chart path/);
  });
});

describe('parseBearerChallenge', () => {
  it('parses the realm/service/scope triplet ghcr.io emits', () => {
    const hdr =
      'Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:stefanprodan/charts/podinfo:pull"';
    expect(parseBearerChallenge(hdr)).toEqual({
      realm: 'https://ghcr.io/token',
      service: 'ghcr.io',
      scope: 'repository:stefanprodan/charts/podinfo:pull',
    });
  });

  it('ignores unknown keys (error, error_description)', () => {
    const hdr = 'Bearer realm="https://auth.example.com",error="invalid_token"';
    expect(parseBearerChallenge(hdr)).toEqual({
      realm: 'https://auth.example.com',
      service: undefined,
      scope: undefined,
    });
  });

  it('returns null for a non-Bearer challenge', () => {
    expect(parseBearerChallenge('Basic realm="x"')).toBeNull();
  });

  it('returns null when realm is absent', () => {
    expect(parseBearerChallenge('Bearer scope="repo:pull"')).toBeNull();
  });
});
