/*
 * Darwin notifications, which is how a simulator is told that a finger was
 * enrolled, or touched the sensor and matched or not: the UI tests post them
 * from inside the simulator, where they run.
 */
#include <notify.h>
