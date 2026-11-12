/*
 * ITSaNAS core, as the iPhone app calls it. Written by hand from
 * crates/itsanas-ios/src/lib.rs; a test of that crate fails when a function
 * is in one and not the other.
 *
 * Every `char *` returned is a JSON envelope, {"ok": "<answer>"} or
 * {"error": "<sentence>"}, and must be given back exactly once with
 * itsanas_free. Every `const char *` taken is a NUL-terminated UTF-8 string
 * that stays valid for the duration of the call; NULL is refused with an
 * error, except where a function says otherwise.
 */
#ifndef ITSANAS_H
#define ITSANAS_H

#include <stdbool.h>
#include <stdint.h>

void itsanas_free(char *answer);

bool itsanas_exists(const char *home);
char *itsanas_passphrase_rule(void);
/* Debug builds only, for the UI tests: the coordinator every node is pinned to. */
char *itsanas_use_coordinator_for_tests(const char *address, const char *device);
char *itsanas_open(const char *home, const char *passphrase);
/* {"opened":false} when the Keychain holds no passphrase. */
char *itsanas_open_remembered(const char *home);
char *itsanas_close(void);

char *itsanas_list_at(const char *folder);
char *itsanas_get(const char *path, const char *destination);
char *itsanas_put(const char *path, const char *source);
char *itsanas_trash_put(const char *path, uint64_t days);
char *itsanas_trash_list(void);
char *itsanas_trash_restore(const char *home, const char *path);
char *itsanas_trash_delete(const char *path);
char *itsanas_sync(bool metadata_only);
char *itsanas_status(void);
bool itsanas_finger_needed(void);
bool itsanas_finger_matches(const char *state);
char *itsanas_background_pass(const char *home, bool metadata_only);

char *itsanas_page_start(const char *home, const char *mode, bool biometric);
char *itsanas_page_stop(void);
char *itsanas_next_request(void);
/* text NULL: the person went Back. */
char *itsanas_reply(uint64_t id, const char *text);

#endif
