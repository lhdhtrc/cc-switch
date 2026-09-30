import { describe, expect, it } from "vitest";
import { getIcon, getIconMetadata, hasIcon, iconList } from "@/icons/extracted";

describe("Antigravity icon", () => {
  it("registers the official mark and metadata in the shared icon catalog", () => {
    const icon = getIcon("antigravity");
    expect(hasIcon("Antigravity")).toBe(true);
    expect(iconList).toContain("antigravity");
    expect(icon).toContain('viewBox="0 0 113 113"');
    expect(icon).toContain("M89.6992 93.695C94.3659 97.195");
    expect(icon).toContain('fill="#3186FF"');
    expect(getIconMetadata("antigravity")).toMatchObject({
      displayName: "Antigravity",
      defaultColor: "#3186FF",
    });
  });
});
