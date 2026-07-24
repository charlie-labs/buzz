import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  cancelManagedDaemon,
  createDaemonBinding,
  createDaemonPackage,
  deleteDaemonBinding,
  deleteDaemonPackage,
  getDaemonPackage,
  getDaemonScheduleStatus,
  listDaemonBindings,
  listDaemonPackages,
  listDaemonRunHistory,
  pickAndImportDaemonFolder,
  pickAndImportDaemonMd,
  runManagedDaemon,
  updateDaemonBinding,
  updateDaemonPackage,
} from "@/shared/api/tauriDaemons";

export const daemonPackagesQueryKey = ["daemon-packages"] as const;
export const daemonBindingsQueryKey = ["daemon-bindings"] as const;
export const daemonHistoryQueryKey = ["daemon-run-history"] as const;
export const daemonPackageQueryKey = (daemonId: string) =>
  ["daemon-package", daemonId] as const;
export const daemonScheduleQueryKey = (bindingId: string) =>
  ["daemon-schedule", bindingId] as const;

function useInvalidateDaemons() {
  const queryClient = useQueryClient();
  return () =>
    Promise.all([
      queryClient.invalidateQueries({ queryKey: daemonPackagesQueryKey }),
      queryClient.invalidateQueries({ queryKey: daemonBindingsQueryKey }),
      queryClient.invalidateQueries({ queryKey: daemonHistoryQueryKey }),
    ]);
}

export function useDaemonPackagesQuery() {
  return useQuery({
    queryKey: daemonPackagesQueryKey,
    queryFn: listDaemonPackages,
  });
}

export function useDaemonPackageQuery(daemonId: string) {
  return useQuery({
    enabled: Boolean(daemonId),
    queryKey: daemonPackageQueryKey(daemonId),
    queryFn: () => getDaemonPackage(daemonId),
    retry: false,
  });
}

export function useDaemonBindingsQuery() {
  return useQuery({
    queryKey: daemonBindingsQueryKey,
    queryFn: listDaemonBindings,
  });
}

export function useDaemonHistoryQuery() {
  return useQuery({
    queryKey: daemonHistoryQueryKey,
    queryFn: listDaemonRunHistory,
    refetchInterval: (query) =>
      query.state.data?.some(
        (record) => record.recordType === "managed" && record.status === null,
      )
        ? 1_000
        : false,
  });
}

export function useDaemonScheduleQuery(bindingId: string) {
  return useQuery({
    enabled: Boolean(bindingId),
    queryKey: daemonScheduleQueryKey(bindingId),
    queryFn: () => getDaemonScheduleStatus(bindingId),
    refetchInterval: 30_000,
  });
}

export function useCreateDaemonPackageMutation() {
  const invalidate = useInvalidateDaemons();
  return useMutation({
    mutationFn: createDaemonPackage,
    onSettled: invalidate,
  });
}

export function useUpdateDaemonPackageMutation(daemonId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: updateDaemonPackage,
    onSettled: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: daemonPackagesQueryKey }),
        queryClient.invalidateQueries({
          queryKey: daemonPackageQueryKey(daemonId),
        }),
      ]);
    },
  });
}

export function useImportDaemonMutation(kind: "file" | "folder") {
  const invalidate = useInvalidateDaemons();
  return useMutation({
    mutationFn: (replace: boolean) =>
      kind === "file"
        ? pickAndImportDaemonMd(replace)
        : pickAndImportDaemonFolder(replace),
    onSettled: invalidate,
  });
}

export function useSaveDaemonBindingMutation(bindingId?: string) {
  const invalidate = useInvalidateDaemons();
  return useMutation({
    mutationFn: (
      request:
        | Parameters<typeof createDaemonBinding>[0]
        | Parameters<typeof updateDaemonBinding>[0],
    ) =>
      bindingId
        ? updateDaemonBinding(
            request as Parameters<typeof updateDaemonBinding>[0],
          )
        : createDaemonBinding(
            request as Parameters<typeof createDaemonBinding>[0],
          ),
    onSettled: invalidate,
  });
}

export function useDeleteDaemonBindingMutation() {
  const invalidate = useInvalidateDaemons();
  return useMutation({
    mutationFn: deleteDaemonBinding,
    onSettled: invalidate,
  });
}

export function useDeleteDaemonPackageMutation() {
  const invalidate = useInvalidateDaemons();
  return useMutation({
    mutationFn: deleteDaemonPackage,
    onSettled: invalidate,
  });
}

export function useRunDaemonMutation(bindingId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: runManagedDaemon,
    onSettled: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: daemonHistoryQueryKey }),
        queryClient.invalidateQueries({
          queryKey: daemonScheduleQueryKey(bindingId),
        }),
      ]);
    },
  });
}

export function useCancelDaemonMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: cancelManagedDaemon,
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: daemonHistoryQueryKey }),
  });
}
