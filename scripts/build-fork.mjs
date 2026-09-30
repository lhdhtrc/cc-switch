#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const localConfigPath = resolve(rootDir, ".antigravity-oauth.local.json");

const ID_KEY = "CC_SWITCH_ANTIGRAVITY_CLIENT_ID";
const SECRET_KEY = "CC_SWITCH_ANTIGRAVITY_CLIENT_SECRET";

function resolveOAuthEnv() {
  const hasIdEnv = Object.prototype.hasOwnProperty.call(process.env, ID_KEY);
  const hasSecretEnv = Object.prototype.hasOwnProperty.call(process.env, SECRET_KEY);

  if (hasIdEnv || hasSecretEnv) {
    if (!hasIdEnv || !hasSecretEnv) {
      console.error(
        `Both ${ID_KEY} and ${SECRET_KEY} must be set together when configuring via environment variables.`,
      );
      process.exit(1);
    }
    const clientId = String(process.env[ID_KEY] ?? "").trim();
    const clientSecret = String(process.env[SECRET_KEY] ?? "").trim();
    if (!clientId || !clientSecret) {
      console.error(`${ID_KEY} and ${SECRET_KEY} must not be empty.`);
      process.exit(1);
    }
    return { [ID_KEY]: clientId, [SECRET_KEY]: clientSecret };
  }

  if (!existsSync(localConfigPath)) {
    console.error(
      `Missing ${localConfigPath}. Copy .antigravity-oauth.example.json to .antigravity-oauth.local.json and fill in ${ID_KEY} and ${SECRET_KEY}, or set both environment variables.`,
    );
    process.exit(1);
  }

  let parsed;
  try {
    parsed = JSON.parse(readFileSync(localConfigPath, "utf8"));
  } catch {
    console.error(`Failed to parse ${localConfigPath} as valid JSON.`);
    process.exit(1);
  }

  const clientId = typeof parsed?.[ID_KEY] === "string" ? parsed[ID_KEY].trim() : "";
  const clientSecret =
    typeof parsed?.[SECRET_KEY] === "string" ? parsed[SECRET_KEY].trim() : "";
  if (!clientId || !clientSecret) {
    console.error(
      `${localConfigPath} must define non-empty string values for ${ID_KEY} and ${SECRET_KEY}.`,
    );
    process.exit(1);
  }

  return { [ID_KEY]: clientId, [SECRET_KEY]: clientSecret };
}

const oauthEnv = resolveOAuthEnv();
const extraArgs = process.argv.slice(2);
const tauriArgs = ["tauri", "build", "--config", "src-tauri/tauri.fork.conf.json", ...extraArgs];

const result = spawnSync("pnpm", tauriArgs, {
  cwd: rootDir,
  stdio: "inherit",
  shell: process.platform === "win32",
  env: {
    ...process.env,
    ...oauthEnv,
  },
});

if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}

process.exit(result.status ?? 1);
