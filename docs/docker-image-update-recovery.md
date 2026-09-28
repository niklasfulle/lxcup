# Docker Compose image update recovery

This procedure applies only to a Compose-managed Linux service updated through
lxcup. The update does not modify the Compose files, but pulling a mutable image
tag can move that tag away from the image that was running before the update.
There is no automatic rollback.

## Before applying an update

Record the old image ID and the exact image reference before confirming the
update. The lxcup audit event `docker.image_update.applied` stores the previous
image ID after a successful update; retaining a separate copy makes recovery
available even if the controller or its audit store is unavailable. Also make
sure the Compose project and its configuration files are backed up and
accessible to the host's Docker user.

## Restore the previous image

Run these commands on the Docker host, substituting values from the trusted
Compose project and the recorded pre-update details:

```sh
OLD_IMAGE_ID='sha256:<previous-config-digest>'
IMAGE_REFERENCE='registry.example/app:stable'
PROJECT='my-compose-project'
SERVICE='app'
WORKING_DIR='/srv/my-compose-project'
COMPOSE_FILE='/srv/my-compose-project/compose.yaml'

docker image inspect "$OLD_IMAGE_ID" >/dev/null
docker image tag "$OLD_IMAGE_ID" "$IMAGE_REFERENCE"
docker compose \
  --project-name "$PROJECT" \
  --project-directory "$WORKING_DIR" \
  --file "$COMPOSE_FILE" \
  up --detach --no-deps --force-recreate --pull never "$SERVICE"
```

Use the same project name, working directory, and Compose file(s) as the
existing service. If the project uses multiple Compose files, include each with
its original `--file` option and order. Check `docker compose ps "$SERVICE"`,
the service health, and application logs after recreation.

If `docker image inspect` reports that the old image is missing, do not retag
the new image as the old one. Restore the exact old image from a trusted backup
or registry reference first and verify its image ID. If the old image is no
longer available, restore the application's data/configuration as required and
deploy a separately reviewed image instead. Digest-pinned Compose references
are not updated by lxcup and should be restored by reverting the Compose
reference to the previously reviewed digest.

This is an operator recovery procedure, not a guarantee that application data
changes made after the update can be reversed. Back up stateful application
data before performing a major-version or otherwise incompatible image update.
