import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "i18next";
import { SubscriptionQuotaView } from "@/components/SubscriptionQuotaFooter";
import type { SubscriptionQuota } from "@/types/subscription";

const quota: SubscriptionQuota = {
  tool: "antigravity",
  credentialStatus: "valid",
  credentialMessage: null,
  success: true,
  tiers: [
    { name: "Gemini Models / 5h", utilization: 18, resetsAt: null },
    { name: "Gemini Models / 5h", utilization: 57, resetsAt: null },
  ],
  extraUsage: null,
  error: null,
  queriedAt: null,
};

describe("SubscriptionQuotaView", () => {
  beforeEach(() => {
    i18n.addResource(
      "zh",
      "translation",
      "subscription.utilization",
      "{{value}}%",
    );
  });

  it.each([false, true])(
    "renders same-name windows without duplicate keys (inline=%s)",
    (inline) => {
      const errors = vi.spyOn(console, "error");
      try {
        render(
          <SubscriptionQuotaView
            quota={quota}
            loading={false}
            refetch={vi.fn()}
            appIdForExpiredHint="Antigravity"
            showUnknownTiers
            inline={inline}
          />,
        );
        expect(screen.getAllByText(/Gemini Models \/ 5h/)).toHaveLength(2);
        expect(screen.getByText("18%")).toBeInTheDocument();
        expect(screen.getByText("57%")).toBeInTheDocument();
        expect(errors).not.toHaveBeenCalled();
      } finally {
        errors.mockRestore();
      }
    },
  );

  it("keeps unknown windows hidden by default for existing callers", () => {
    const { container } = render(
      <SubscriptionQuotaView
        quota={quota}
        loading={false}
        refetch={vi.fn()}
        appIdForExpiredHint="gemini"
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });
});
