import { useCallback, useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  antigravityAuthApi,
  type AntigravityLogin,
} from "@/lib/api/antigravityAuth";
import { settingsApi } from "@/lib/api";
import { extractErrorMessage } from "@/utils/errorUtils";

export const antigravityAuthKeys = {
  status: ["antigravity_oauth", "status"] as const,
  quotas: ["antigravity_oauth", "quota"] as const,
  quota: (accountId: string) =>
    ["antigravity_oauth", "quota", accountId] as const,
};

export function useAntigravityOauth() {
  const client = useQueryClient();
  const [login, setLogin] = useState<AntigravityLogin | null>(null);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
  const activeLogin = useRef<AntigravityLogin | null>(null);
  const mounted = useRef(true);
  const status = useQuery({
    queryKey: antigravityAuthKeys.status,
    queryFn: antigravityAuthApi.status,
    staleTime: 30_000,
    retry: 1,
  });
  const poll = useQuery({
    queryKey: ["antigravity_oauth", "login", login?.flowId],
    queryFn: () => antigravityAuthApi.poll(login!.flowId),
    enabled: !!login,
    refetchInterval: login ? 1500 : false,
    refetchIntervalInBackground: true,
    retry: 1,
  });

  const cancelFlow = useCallback(
    async (flowId: string) => {
      try {
        await antigravityAuthApi.cancel(flowId);
      } finally {
        // The server may have saved the account before cancellation reached it.
        void client.invalidateQueries({ queryKey: antigravityAuthKeys.status });
        void client.resetQueries({ queryKey: antigravityAuthKeys.quotas });
      }
    },
    [client],
  );

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      generation.current++;
      if (activeLogin.current) {
        void cancelFlow(activeLogin.current.flowId).catch(() => {});
        activeLogin.current = null;
      }
    };
  }, [cancelFlow]);

  useEffect(() => {
    if (!login || !poll.data || poll.data.status === "pending") return;
    if (poll.data.status === "completed") {
      void client.invalidateQueries({ queryKey: antigravityAuthKeys.status });
      void client.resetQueries({
        queryKey: antigravityAuthKeys.quota(poll.data.account.id),
      });
      setError(null);
    } else {
      setError(poll.data.message);
    }
    activeLogin.current = null;
    setLogin(null);
  }, [client, login, poll.data]);

  const start = useCallback(
    async (targetAccountId: string | null = null) => {
      const currentGeneration = ++generation.current;
      setStarting(true);
      setError(null);
      try {
        const result = await antigravityAuthApi.start(targetAccountId);
        if (!mounted.current || currentGeneration !== generation.current) {
          await cancelFlow(result.flowId);
          return;
        }
        activeLogin.current = result;
        setLogin(result);
        try {
          await settingsApi.openExternal(result.authorizationUrl);
        } catch (cause) {
          if (mounted.current && currentGeneration === generation.current) {
            setError(extractErrorMessage(cause));
          }
        }
      } catch (cause) {
        if (mounted.current && currentGeneration === generation.current) {
          setError(extractErrorMessage(cause));
        }
      } finally {
        if (mounted.current && currentGeneration === generation.current) {
          setStarting(false);
        }
      }
    },
    [cancelFlow],
  );

  const cancel = async () => {
    const currentGeneration = ++generation.current;
    const current = activeLogin.current;
    activeLogin.current = null;
    setLogin(null);
    setStarting(false);
    setError(null);
    if (current) {
      try {
        await cancelFlow(current.flowId);
      } catch (cause) {
        if (mounted.current && currentGeneration === generation.current) {
          setError(extractErrorMessage(cause));
        }
      }
    }
  };
  const update = useMutation({
    mutationFn: async ({
      accountId,
      remove,
    }: {
      accountId: string;
      remove: boolean;
    }) => {
      if (remove) await antigravityAuthApi.remove(accountId);
      else await antigravityAuthApi.setDefault(accountId);
      return { accountId, remove };
    },
    onSuccess: ({ accountId, remove }) => {
      if (remove)
        client.removeQueries({
          queryKey: antigravityAuthKeys.quota(accountId),
        });
      void client.invalidateQueries({ queryKey: antigravityAuthKeys.status });
      setError(null);
    },
    onError: (cause) => setError(extractErrorMessage(cause)),
  });

  const openBrowser = async () => {
    if (!login) return;
    try {
      await settingsApi.openExternal(login.authorizationUrl);
      if (mounted.current) setError(null);
    } catch (cause) {
      if (mounted.current) setError(extractErrorMessage(cause));
    }
  };

  return {
    status,
    login,
    starting,
    busy: starting || !!login || update.isPending,
    error: error || (poll.isError ? extractErrorMessage(poll.error) : null),
    start,
    cancel,
    openBrowser,
    remove: (accountId: string) => update.mutate({ accountId, remove: true }),
    setDefault: (accountId: string) =>
      update.mutate({ accountId, remove: false }),
  };
}
