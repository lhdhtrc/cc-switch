import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "i18next";
import zh from "@/i18n/locales/zh.json";
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
    i18n.addResourceBundle("zh", "translation", zh, true, true);
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
        expect(screen.getByText(/82%/)).toBeInTheDocument();
        expect(screen.getByText(/43%/)).toBeInTheDocument();
        if (!inline) {
          const meters = screen.getAllByRole("meter");
          expect(meters[0]).toHaveAttribute("aria-valuenow", "82");
          expect(meters[1]).toHaveAttribute("aria-valuenow", "43");
        }
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
