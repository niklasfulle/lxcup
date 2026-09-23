-- Only the platform API configuration is retired here. Historical LXC and
-- Docker records stay available while their target-based successor is rolled
-- out, so an upgrade never destroys managed workload history.
DROP TABLE IF EXISTS proxmox_environments;
