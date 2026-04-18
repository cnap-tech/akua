import { describe, it, expect } from 'vitest';

import { validateHost, SsrfError } from '../src/ssrf.ts';
import { AkuaError } from '../src/errors.ts';

describe('validateHost', () => {
  it('rejects AWS metadata IP', () => {
    expect(() => validateHost('169.254.169.254')).toThrow(SsrfError);
  });

  it('rejects loopback IPv4 / IPv6', () => {
    expect(() => validateHost('127.0.0.1')).toThrow(SsrfError);
    expect(() => validateHost('127.0.0.1:8080')).toThrow(SsrfError);
    expect(() => validateHost('[::1]')).toThrow(SsrfError);
  });

  it('rejects RFC1918', () => {
    expect(() => validateHost('10.0.0.1')).toThrow(SsrfError);
    expect(() => validateHost('172.16.5.5')).toThrow(SsrfError);
    expect(() => validateHost('192.168.1.1')).toThrow(SsrfError);
  });

  it('allows public IPs', () => {
    expect(() => validateHost('8.8.8.8')).not.toThrow();
  });

  it('allows DNS names', () => {
    expect(() => validateHost('ghcr.io')).not.toThrow();
    expect(() => validateHost('charts.bitnami.com')).not.toThrow();
    expect(() => validateHost('registry.example.com:5000')).not.toThrow();
  });

  it('SsrfError extends AkuaError', () => {
    try {
      validateHost('127.0.0.1');
    } catch (err) {
      expect(err).toBeInstanceOf(AkuaError);
    }
  });

  it('AKUA_ALLOW_PRIVATE_HOSTS=1 bypasses the check', () => {
    const prior = process.env.AKUA_ALLOW_PRIVATE_HOSTS;
    process.env.AKUA_ALLOW_PRIVATE_HOSTS = '1';
    try {
      expect(() => validateHost('127.0.0.1')).not.toThrow();
    } finally {
      if (prior === undefined) delete process.env.AKUA_ALLOW_PRIVATE_HOSTS;
      else process.env.AKUA_ALLOW_PRIVATE_HOSTS = prior;
    }
  });
});
