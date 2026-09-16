import { useQuery } from "@tanstack/react-query";
import { apiClient, type ContainerDto, type NodeDto } from "./api";

export const queryKeys = {
  nodes: ["nodes"] as const,
  containers: ["containers"] as const,
  nodeContainers: (nodeId: string) => ["nodes", nodeId, "containers"] as const,
};

export function useNodes() {
  return useQuery<NodeDto[]>({
    queryKey: queryKeys.nodes,
    queryFn: ({ signal }) => apiClient.get<NodeDto[]>("/api/v1/nodes", signal),
    staleTime: 15_000,
  });
}

export function useContainers() {
  return useQuery<ContainerDto[]>({
    queryKey: queryKeys.containers,
    queryFn: ({ signal }) => apiClient.get<ContainerDto[]>("/api/v1/containers", signal),
    staleTime: 15_000,
  });
}

export function useNodeContainers(nodeId: string | undefined) {
  return useQuery<ContainerDto[]>({
    queryKey: nodeId ? queryKeys.nodeContainers(nodeId) : ["nodes", "none", "containers"],
    queryFn: ({ signal }) => apiClient.get<ContainerDto[]>(`/api/v1/nodes/${nodeId}/containers`, signal),
    enabled: Boolean(nodeId),
    staleTime: 15_000,
  });
}
