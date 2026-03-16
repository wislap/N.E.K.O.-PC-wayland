#include <locale.h>
#include <stdatomic.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "include/capi/cef_app_capi.h"
#include "include/capi/cef_browser_capi.h"
#include "include/capi/cef_browser_process_handler_capi.h"
#include "include/capi/cef_client_capi.h"
#include "include/capi/cef_command_line_capi.h"
#include "include/capi/cef_display_handler_capi.h"
#include "include/capi/cef_life_span_handler_capi.h"
#include "include/capi/cef_load_handler_capi.h"
#include "include/capi/cef_render_handler_capi.h"
#include "include/capi/cef_scheme_capi.h"
#include "include/cef_api_hash.h"

typedef struct _neko_browser_process_handler_t {
  cef_browser_process_handler_t handler;
  atomic_int ref_count;
} neko_browser_process_handler_t;

typedef struct _neko_app_t {
  cef_app_t app;
  atomic_int ref_count;
  neko_browser_process_handler_t* browser_process_handler;
} neko_app_t;

typedef struct _neko_cef_runtime_t neko_cef_runtime_t;
typedef struct _neko_cef_browser_t neko_cef_browser_t;

typedef void (*neko_cef_on_after_created_cb)(void* user_data);
typedef void (*neko_cef_on_before_close_cb)(void* user_data);
typedef void (*neko_cef_on_loading_state_change_cb)(void* user_data,
                                                    int is_loading,
                                                    int can_go_back,
                                                    int can_go_forward);
typedef void (*neko_cef_on_load_start_cb)(void* user_data, int transition_type);
typedef void (*neko_cef_on_load_end_cb)(void* user_data, int http_status_code);
typedef void (*neko_cef_on_load_error_cb)(void* user_data,
                                          int error_code,
                                          const char* error_text,
                                          const char* failed_url);
typedef void (*neko_cef_on_console_cb)(void* user_data,
                                       int level,
                                       const char* source,
                                       int line,
                                       const char* message);
typedef void (*neko_cef_on_paint_cb)(void* user_data,
                                     int element_type,
                                     const void* buffer,
                                     int width,
                                     int height);

typedef struct _neko_cef_runtime_settings_t {
  const char* browser_subprocess_path;
  const char* resources_dir_path;
  const char* locales_dir_path;
  const char* locale;
  const char* cache_path;
  const char* root_cache_path;
  int no_sandbox;
  int multi_threaded_message_loop;
  int windowless_rendering_enabled;
  int external_message_pump;
  int remote_debugging_port;
  int use_app;
} neko_cef_runtime_settings_t;

typedef struct _neko_cef_browser_config_t {
  const char* url;
  const char* window_name;
  int width;
  int height;
  int frame_rate;
  int transparent_painting;
} neko_cef_browser_config_t;

typedef struct _neko_cef_browser_callbacks_t {
  neko_cef_on_after_created_cb on_after_created;
  neko_cef_on_before_close_cb on_before_close;
  neko_cef_on_loading_state_change_cb on_loading_state_change;
  neko_cef_on_load_start_cb on_load_start;
  neko_cef_on_load_end_cb on_load_end;
  neko_cef_on_load_error_cb on_load_error;
  neko_cef_on_console_cb on_console;
  neko_cef_on_paint_cb on_paint;
} neko_cef_browser_callbacks_t;

struct _neko_cef_runtime_t {
  neko_app_t* app;
  int initialized;
  int message_loop_mode;
};

struct _neko_cef_browser_t {
  atomic_int ref_count;
  neko_cef_runtime_t* runtime;
  void* user_data;
  neko_cef_browser_callbacks_t callbacks;
  int width;
  int height;
  int transparent_painting;
  atomic_int closed;
  cef_browser_t* browser;
  cef_browser_host_t* host;

  cef_client_t client;
  cef_display_handler_t display_handler;
  cef_life_span_handler_t life_span_handler;
  cef_load_handler_t load_handler;
  cef_render_handler_t render_handler;
};

enum {
  NEKO_CEF_MESSAGE_LOOP_EXTERNAL_PUMP = 0,
  NEKO_CEF_MESSAGE_LOOP_MULTI_THREADED = 1,
};

enum {
  NEKO_CEF_MOUSE_BUTTON_LEFT = 0,
  NEKO_CEF_MOUSE_BUTTON_MIDDLE = 1,
  NEKO_CEF_MOUSE_BUTTON_RIGHT = 2,
};

enum {
  NEKO_CEF_KEY_EVENT_RAW_KEY_DOWN = 0,
  NEKO_CEF_KEY_EVENT_KEY_UP = 1,
  NEKO_CEF_KEY_EVENT_CHAR = 2,
};

#define NEKO_CEF_LOG(...)    \
  do {                       \
    fprintf(stderr, __VA_ARGS__); \
    fflush(stderr);          \
  } while (0)

static void neko_browser_process_handler_add_ref(cef_base_ref_counted_t* self) {
  neko_browser_process_handler_t* obj = (neko_browser_process_handler_t*)self;
  atomic_fetch_add(&obj->ref_count, 1);
}

static int neko_browser_process_handler_release(cef_base_ref_counted_t* self) {
  neko_browser_process_handler_t* obj = (neko_browser_process_handler_t*)self;
  int count = atomic_fetch_sub(&obj->ref_count, 1) - 1;
  if (count == 0) {
    free(obj);
    return 1;
  }
  return 0;
}

static int neko_browser_process_handler_has_one_ref(cef_base_ref_counted_t* self) {
  neko_browser_process_handler_t* obj = (neko_browser_process_handler_t*)self;
  return atomic_load(&obj->ref_count) == 1;
}

static int neko_browser_process_handler_has_at_least_one_ref(cef_base_ref_counted_t* self) {
  neko_browser_process_handler_t* obj = (neko_browser_process_handler_t*)self;
  return atomic_load(&obj->ref_count) >= 1;
}

static void neko_app_add_ref(cef_base_ref_counted_t* self) {
  neko_app_t* obj = (neko_app_t*)self;
  atomic_fetch_add(&obj->ref_count, 1);
}

static int neko_app_release(cef_base_ref_counted_t* self) {
  neko_app_t* obj = (neko_app_t*)self;
  int count = atomic_fetch_sub(&obj->ref_count, 1) - 1;
  if (count == 0) {
    if (obj->browser_process_handler) {
      obj->browser_process_handler->handler.base.release(
          &obj->browser_process_handler->handler.base);
    }
    free(obj);
    return 1;
  }
  return 0;
}

static int neko_app_has_one_ref(cef_base_ref_counted_t* self) {
  neko_app_t* obj = (neko_app_t*)self;
  return atomic_load(&obj->ref_count) == 1;
}

static int neko_app_has_at_least_one_ref(cef_base_ref_counted_t* self) {
  neko_app_t* obj = (neko_app_t*)self;
  return atomic_load(&obj->ref_count) >= 1;
}

static void neko_on_before_command_line_processing(
    cef_app_t* self,
    const cef_string_t* process_type,
    cef_command_line_t* command_line) {
  (void)self;
  (void)process_type;
  (void)command_line;
}

static cef_browser_process_handler_t* neko_get_browser_process_handler(
    cef_app_t* self) {
  neko_app_t* app = (neko_app_t*)self;
  if (!app->browser_process_handler) {
    return NULL;
  }
  app->browser_process_handler->handler.base.add_ref(
      &app->browser_process_handler->handler.base);
  return &app->browser_process_handler->handler;
}

static neko_browser_process_handler_t* neko_browser_process_handler_create(void) {
  neko_browser_process_handler_t* handler =
      (neko_browser_process_handler_t*)calloc(1, sizeof(neko_browser_process_handler_t));
  if (!handler) {
    return NULL;
  }

  handler->handler.base.size = sizeof(cef_browser_process_handler_t);
  handler->handler.base.add_ref = neko_browser_process_handler_add_ref;
  handler->handler.base.release = neko_browser_process_handler_release;
  handler->handler.base.has_one_ref = neko_browser_process_handler_has_one_ref;
  handler->handler.base.has_at_least_one_ref =
      neko_browser_process_handler_has_at_least_one_ref;
  atomic_store(&handler->ref_count, 1);
  return handler;
}

static neko_app_t* neko_app_create(void) {
  neko_app_t* app = (neko_app_t*)calloc(1, sizeof(neko_app_t));
  if (!app) {
    return NULL;
  }

  app->app.base.size = sizeof(cef_app_t);
  app->app.base.add_ref = neko_app_add_ref;
  app->app.base.release = neko_app_release;
  app->app.base.has_one_ref = neko_app_has_one_ref;
  app->app.base.has_at_least_one_ref = neko_app_has_at_least_one_ref;
  app->app.get_browser_process_handler = neko_get_browser_process_handler;
  app->browser_process_handler = neko_browser_process_handler_create();
  atomic_store(&app->ref_count, 1);
  return app;
}

static void neko_cef_write_error(char* buffer, size_t capacity, const char* message) {
  if (!buffer || capacity == 0) {
    return;
  }

  if (!message) {
    buffer[0] = '\0';
    return;
  }

  snprintf(buffer, capacity, "%s", message);
}

static int neko_cef_join_path(char* dest,
                              size_t dest_capacity,
                              const char* base,
                              const char* suffix) {
  int written;
  if (!dest || dest_capacity == 0 || !base || !suffix) {
    return 0;
  }
  written = snprintf(dest, dest_capacity, "%s%s", base, suffix);
  return written > 0 && (size_t)written < dest_capacity;
}

static const char* neko_cef_pick_string(const char* value, const char* fallback) {
  if (value && value[0] != '\0') {
    return value;
  }
  return fallback;
}

static void neko_cef_set_cef_string(cef_string_t* dest, const char* value) {
  if (!dest) {
    return;
  }
  memset(dest, 0, sizeof(*dest));
  if (value && value[0] != '\0') {
    cef_string_from_ascii(value, strlen(value), dest);
  }
}

static void neko_cef_clear_cef_string(cef_string_t* value) {
  if (!value) {
    return;
  }
  cef_string_utf16_clear(value);
}

static char* neko_cef_strdup_utf8(const cef_string_t* value) {
  cef_string_utf8_t utf8 = {};
  char* copy = NULL;

  if (!value || !value->str || value->length == 0) {
    copy = (char*)calloc(1, 1);
    return copy;
  }

  if (!cef_string_utf16_to_utf8(value->str, value->length, &utf8)) {
    copy = (char*)calloc(1, 1);
    return copy;
  }

  copy = (char*)calloc(utf8.length + 1, 1);
  if (copy && utf8.str && utf8.length > 0) {
    memcpy(copy, utf8.str, utf8.length);
  }
  cef_string_utf8_clear(&utf8);
  return copy;
}

static void neko_cef_browser_add_ref(neko_cef_browser_t* bridge) {
  if (bridge) {
    atomic_fetch_add(&bridge->ref_count, 1);
  }
}

static void neko_cef_browser_free(neko_cef_browser_t* bridge) {
  if (!bridge) {
    return;
  }

  if (bridge->host) {
    bridge->host->base.release(&bridge->host->base);
    bridge->host = NULL;
  }
  if (bridge->browser) {
    bridge->browser->base.release(&bridge->browser->base);
    bridge->browser = NULL;
  }
  free(bridge);
}

static int neko_cef_browser_release_ref(neko_cef_browser_t* bridge) {
  if (!bridge) {
    return 0;
  }
  int count = atomic_fetch_sub(&bridge->ref_count, 1) - 1;
  if (count == 0) {
    neko_cef_browser_free(bridge);
    return 1;
  }
  return 0;
}

#define NEKO_CEF_DEFINE_BRIDGE_BASE_FUNCS(name, field_type, field_name)                            \
  static neko_cef_browser_t* name##_bridge_from_##field_name(field_type* self) {                  \
    return (neko_cef_browser_t*)((char*)self - offsetof(neko_cef_browser_t, field_name));         \
  }                                                                                                \
                                                                                                   \
  static void name##_add_ref(cef_base_ref_counted_t* self) {                                       \
    if (!self) {                                                                                   \
      return;                                                                                      \
    }                                                                                              \
    neko_cef_browser_add_ref(name##_bridge_from_##field_name((field_type*)self));                 \
  }                                                                                                \
                                                                                                   \
  static int name##_release(cef_base_ref_counted_t* self) {                                        \
    if (!self) {                                                                                   \
      return 0;                                                                                    \
    }                                                                                              \
    return neko_cef_browser_release_ref(                                                           \
        name##_bridge_from_##field_name((field_type*)self));                                       \
  }                                                                                                \
                                                                                                   \
  static int name##_has_one_ref(cef_base_ref_counted_t* self) {                                    \
    if (!self) {                                                                                   \
      return 0;                                                                                    \
    }                                                                                              \
    neko_cef_browser_t* bridge = name##_bridge_from_##field_name((field_type*)self);              \
    return atomic_load(&bridge->ref_count) == 1;                                                   \
  }                                                                                                \
                                                                                                   \
  static int name##_has_at_least_one_ref(cef_base_ref_counted_t* self) {                           \
    if (!self) {                                                                                   \
      return 0;                                                                                    \
    }                                                                                              \
    neko_cef_browser_t* bridge = name##_bridge_from_##field_name((field_type*)self);              \
    return atomic_load(&bridge->ref_count) >= 1;                                                   \
  }

NEKO_CEF_DEFINE_BRIDGE_BASE_FUNCS(neko_cef_client_base, cef_client_t, client)
NEKO_CEF_DEFINE_BRIDGE_BASE_FUNCS(neko_cef_display_base,
                                  cef_display_handler_t,
                                  display_handler)
NEKO_CEF_DEFINE_BRIDGE_BASE_FUNCS(neko_cef_life_base,
                                  cef_life_span_handler_t,
                                  life_span_handler)
NEKO_CEF_DEFINE_BRIDGE_BASE_FUNCS(neko_cef_load_base, cef_load_handler_t, load_handler)
NEKO_CEF_DEFINE_BRIDGE_BASE_FUNCS(neko_cef_render_base,
                                  cef_render_handler_t,
                                  render_handler)

static void neko_cef_browser_capture_objects(neko_cef_browser_t* bridge,
                                             cef_browser_t* browser) {
  if (!bridge || !browser) {
    return;
  }

  if (!bridge->browser) {
    browser->base.add_ref(&browser->base);
    bridge->browser = browser;
  }

  if (!bridge->host && browser->get_host) {
    cef_browser_host_t* host = browser->get_host(browser);
    if (host) {
      host->base.add_ref(&host->base);
      bridge->host = host;
    }
  }
}

static cef_display_handler_t* neko_cef_client_get_display_handler(cef_client_t* self) {
  neko_cef_browser_t* bridge = neko_cef_client_base_bridge_from_client(self);
  bridge->display_handler.base.add_ref(&bridge->display_handler.base);
  return &bridge->display_handler;
}

static cef_life_span_handler_t* neko_cef_client_get_life_span_handler(cef_client_t* self) {
  neko_cef_browser_t* bridge = neko_cef_client_base_bridge_from_client(self);
  bridge->life_span_handler.base.add_ref(&bridge->life_span_handler.base);
  return &bridge->life_span_handler;
}

static cef_load_handler_t* neko_cef_client_get_load_handler(cef_client_t* self) {
  neko_cef_browser_t* bridge = neko_cef_client_base_bridge_from_client(self);
  bridge->load_handler.base.add_ref(&bridge->load_handler.base);
  return &bridge->load_handler;
}

static cef_render_handler_t* neko_cef_client_get_render_handler(cef_client_t* self) {
  neko_cef_browser_t* bridge = neko_cef_client_base_bridge_from_client(self);
  bridge->render_handler.base.add_ref(&bridge->render_handler.base);
  return &bridge->render_handler;
}

static int neko_cef_life_on_before_popup(cef_life_span_handler_t* self,
                                         cef_browser_t* browser,
                                         cef_frame_t* frame,
                                         int popup_id,
                                         const cef_string_t* target_url,
                                         const cef_string_t* target_frame_name,
                                         cef_window_open_disposition_t target_disposition,
                                         int user_gesture,
                                         const cef_popup_features_t* popup_features,
                                         cef_window_info_t* window_info,
                                         cef_client_t** client,
                                         cef_browser_settings_t* settings,
                                         cef_dictionary_value_t** extra_info,
                                         int* no_javascript_access) {
  (void)self;
  (void)browser;
  (void)frame;
  (void)popup_id;
  (void)target_url;
  (void)target_frame_name;
  (void)target_disposition;
  (void)user_gesture;
  (void)popup_features;
  (void)window_info;
  (void)client;
  (void)settings;
  (void)extra_info;
  (void)no_javascript_access;
  return 1;
}

static void neko_cef_life_on_after_created(cef_life_span_handler_t* self,
                                           cef_browser_t* browser) {
  neko_cef_browser_t* bridge = neko_cef_life_base_bridge_from_life_span_handler(self);
  neko_cef_browser_capture_objects(bridge, browser);
  if (bridge->callbacks.on_after_created) {
    bridge->callbacks.on_after_created(bridge->user_data);
  }
}

static int neko_cef_life_do_close(cef_life_span_handler_t* self, cef_browser_t* browser) {
  (void)self;
  (void)browser;
  return 0;
}

static void neko_cef_life_on_before_close(cef_life_span_handler_t* self,
                                          cef_browser_t* browser) {
  neko_cef_browser_t* bridge = neko_cef_life_base_bridge_from_life_span_handler(self);
  (void)browser;
  atomic_store(&bridge->closed, 1);
  if (bridge->callbacks.on_before_close) {
    bridge->callbacks.on_before_close(bridge->user_data);
  }

  if (bridge->host) {
    bridge->host->base.release(&bridge->host->base);
    bridge->host = NULL;
  }
  if (bridge->browser) {
    bridge->browser->base.release(&bridge->browser->base);
    bridge->browser = NULL;
  }
}

static int neko_cef_display_on_console_message(cef_display_handler_t* self,
                                               cef_browser_t* browser,
                                               cef_log_severity_t level,
                                               const cef_string_t* message,
                                               const cef_string_t* source,
                                               int line) {
  neko_cef_browser_t* bridge = neko_cef_display_base_bridge_from_display_handler(self);
  (void)browser;
  if (bridge->callbacks.on_console) {
    char* message_utf8 = neko_cef_strdup_utf8(message);
    char* source_utf8 = neko_cef_strdup_utf8(source);
    bridge->callbacks.on_console(bridge->user_data,
                                 level,
                                 source_utf8 ? source_utf8 : "",
                                 line,
                                 message_utf8 ? message_utf8 : "");
    free(message_utf8);
    free(source_utf8);
  }
  return 0;
}

static void neko_cef_load_on_loading_state_change(cef_load_handler_t* self,
                                                  cef_browser_t* browser,
                                                  int is_loading,
                                                  int can_go_back,
                                                  int can_go_forward) {
  neko_cef_browser_t* bridge = neko_cef_load_base_bridge_from_load_handler(self);
  (void)browser;
  if (bridge->callbacks.on_loading_state_change) {
    bridge->callbacks.on_loading_state_change(
        bridge->user_data, is_loading, can_go_back, can_go_forward);
  }
}

static void neko_cef_load_on_load_start(cef_load_handler_t* self,
                                        cef_browser_t* browser,
                                        cef_frame_t* frame,
                                        cef_transition_type_t transition_type) {
  neko_cef_browser_t* bridge = neko_cef_load_base_bridge_from_load_handler(self);
  (void)browser;
  (void)frame;
  if (bridge->callbacks.on_load_start) {
    bridge->callbacks.on_load_start(bridge->user_data, transition_type);
  }
}

static void neko_cef_load_on_load_end(cef_load_handler_t* self,
                                      cef_browser_t* browser,
                                      cef_frame_t* frame,
                                      int http_status_code) {
  neko_cef_browser_t* bridge = neko_cef_load_base_bridge_from_load_handler(self);
  (void)browser;
  (void)frame;
  if (bridge->callbacks.on_load_end) {
    bridge->callbacks.on_load_end(bridge->user_data, http_status_code);
  }
}

static void neko_cef_load_on_load_error(cef_load_handler_t* self,
                                        cef_browser_t* browser,
                                        cef_frame_t* frame,
                                        cef_errorcode_t error_code,
                                        const cef_string_t* error_text,
                                        const cef_string_t* failed_url) {
  neko_cef_browser_t* bridge = neko_cef_load_base_bridge_from_load_handler(self);
  (void)browser;
  (void)frame;
  if (bridge->callbacks.on_load_error) {
    char* error_text_utf8 = neko_cef_strdup_utf8(error_text);
    char* failed_url_utf8 = neko_cef_strdup_utf8(failed_url);
    bridge->callbacks.on_load_error(bridge->user_data,
                                    error_code,
                                    error_text_utf8 ? error_text_utf8 : "",
                                    failed_url_utf8 ? failed_url_utf8 : "");
    free(error_text_utf8);
    free(failed_url_utf8);
  }
}

static void neko_cef_render_get_view_rect(cef_render_handler_t* self,
                                          cef_browser_t* browser,
                                          cef_rect_t* rect) {
  neko_cef_browser_t* bridge = neko_cef_render_base_bridge_from_render_handler(self);
  neko_cef_browser_capture_objects(bridge, browser);
  if (!rect) {
    return;
  }
  rect->x = 0;
  rect->y = 0;
  rect->width = bridge->width;
  rect->height = bridge->height;
}

static int neko_cef_render_get_screen_info(cef_render_handler_t* self,
                                           cef_browser_t* browser,
                                           cef_screen_info_t* screen_info) {
  neko_cef_browser_t* bridge = neko_cef_render_base_bridge_from_render_handler(self);
  neko_cef_browser_capture_objects(bridge, browser);
  if (!screen_info) {
    return 0;
  }

  memset(screen_info, 0, sizeof(*screen_info));
#if CEF_API_ADDED(12000)
  screen_info->size = sizeof(*screen_info);
#endif
  screen_info->device_scale_factor = 1.0f;
  screen_info->depth = 32;
  screen_info->depth_per_component = 8;
  screen_info->is_monochrome = 0;
  screen_info->rect.x = 0;
  screen_info->rect.y = 0;
  screen_info->rect.width = bridge->width;
  screen_info->rect.height = bridge->height;
  screen_info->available_rect = screen_info->rect;
  return 1;
}

static void neko_cef_render_on_paint(cef_render_handler_t* self,
                                     cef_browser_t* browser,
                                     cef_paint_element_type_t type,
                                     size_t dirty_rects_count,
                                     const cef_rect_t* dirty_rects,
                                     const void* buffer,
                                     int width,
                                     int height) {
  neko_cef_browser_t* bridge = neko_cef_render_base_bridge_from_render_handler(self);
  (void)dirty_rects_count;
  (void)dirty_rects;
  neko_cef_browser_capture_objects(bridge, browser);
  if (bridge->callbacks.on_paint) {
    bridge->callbacks.on_paint(bridge->user_data, type, buffer, width, height);
  }
}

static neko_cef_browser_t* neko_cef_browser_create(neko_cef_runtime_t* runtime,
                                                   const neko_cef_browser_callbacks_t* callbacks,
                                                   void* user_data) {
  neko_cef_browser_t* bridge =
      (neko_cef_browser_t*)calloc(1, sizeof(neko_cef_browser_t));
  if (!bridge) {
    return NULL;
  }

  atomic_store(&bridge->ref_count, 1);
  atomic_store(&bridge->closed, 0);
  bridge->runtime = runtime;
  bridge->user_data = user_data;
  if (callbacks) {
    bridge->callbacks = *callbacks;
  }

  bridge->client.base.size = sizeof(cef_client_t);
  bridge->client.base.add_ref = neko_cef_client_base_add_ref;
  bridge->client.base.release = neko_cef_client_base_release;
  bridge->client.base.has_one_ref = neko_cef_client_base_has_one_ref;
  bridge->client.base.has_at_least_one_ref = neko_cef_client_base_has_at_least_one_ref;
  bridge->client.get_display_handler = neko_cef_client_get_display_handler;
  bridge->client.get_life_span_handler = neko_cef_client_get_life_span_handler;
  bridge->client.get_load_handler = neko_cef_client_get_load_handler;
  bridge->client.get_render_handler = neko_cef_client_get_render_handler;

  bridge->display_handler.base.size = sizeof(cef_display_handler_t);
  bridge->display_handler.base.add_ref = neko_cef_display_base_add_ref;
  bridge->display_handler.base.release = neko_cef_display_base_release;
  bridge->display_handler.base.has_one_ref = neko_cef_display_base_has_one_ref;
  bridge->display_handler.base.has_at_least_one_ref =
      neko_cef_display_base_has_at_least_one_ref;
  bridge->display_handler.on_console_message = neko_cef_display_on_console_message;

  bridge->life_span_handler.base.size = sizeof(cef_life_span_handler_t);
  bridge->life_span_handler.base.add_ref = neko_cef_life_base_add_ref;
  bridge->life_span_handler.base.release = neko_cef_life_base_release;
  bridge->life_span_handler.base.has_one_ref = neko_cef_life_base_has_one_ref;
  bridge->life_span_handler.base.has_at_least_one_ref =
      neko_cef_life_base_has_at_least_one_ref;
  bridge->life_span_handler.on_before_popup = neko_cef_life_on_before_popup;
  bridge->life_span_handler.on_after_created = neko_cef_life_on_after_created;
  bridge->life_span_handler.do_close = neko_cef_life_do_close;
  bridge->life_span_handler.on_before_close = neko_cef_life_on_before_close;

  bridge->load_handler.base.size = sizeof(cef_load_handler_t);
  bridge->load_handler.base.add_ref = neko_cef_load_base_add_ref;
  bridge->load_handler.base.release = neko_cef_load_base_release;
  bridge->load_handler.base.has_one_ref = neko_cef_load_base_has_one_ref;
  bridge->load_handler.base.has_at_least_one_ref = neko_cef_load_base_has_at_least_one_ref;
  bridge->load_handler.on_loading_state_change = neko_cef_load_on_loading_state_change;
  bridge->load_handler.on_load_start = neko_cef_load_on_load_start;
  bridge->load_handler.on_load_end = neko_cef_load_on_load_end;
  bridge->load_handler.on_load_error = neko_cef_load_on_load_error;

  bridge->render_handler.base.size = sizeof(cef_render_handler_t);
  bridge->render_handler.base.add_ref = neko_cef_render_base_add_ref;
  bridge->render_handler.base.release = neko_cef_render_base_release;
  bridge->render_handler.base.has_one_ref = neko_cef_render_base_has_one_ref;
  bridge->render_handler.base.has_at_least_one_ref =
      neko_cef_render_base_has_at_least_one_ref;
  bridge->render_handler.get_view_rect = neko_cef_render_get_view_rect;
  bridge->render_handler.get_screen_info = neko_cef_render_get_screen_info;
  bridge->render_handler.on_paint = neko_cef_render_on_paint;

  return bridge;
}

int neko_cef_bridge_execute_process(int argc, char** argv, int use_app) {
  cef_api_hash(CEF_API_VERSION, 0);
  NEKO_CEF_LOG("NEKO_CEF_C execute_process begin argc=%d use_app=%d argv0=%s\n",
               argc,
               use_app,
               (argc > 0 && argv && argv[0]) ? argv[0] : "<null>");

  cef_main_args_t main_args;
  memset(&main_args, 0, sizeof(main_args));
  main_args.argc = argc;
  main_args.argv = argv;

  neko_app_t* app = NULL;
  cef_app_t* cef_app = NULL;
  if (use_app) {
    app = neko_app_create();
    if (!app) {
      return 91;
    }
    cef_app = &app->app;
    app->app.base.add_ref(&app->app.base);
  }

  int exit_code = cef_execute_process(&main_args, cef_app, NULL);
  NEKO_CEF_LOG("NEKO_CEF_C execute_process returned %d\n", exit_code);

  if (app) {
    app->app.base.release(&app->app.base);
  }

  return exit_code;
}

neko_cef_runtime_t* neko_cef_bridge_initialize(int argc,
                                               char** argv,
                                               const neko_cef_runtime_settings_t* options,
                                               char* error_message,
                                               size_t error_message_capacity) {
  cef_main_args_t main_args;
  cef_settings_t settings;
  cef_string_t browser_subprocess_path = {};
  cef_string_t resources_dir_path = {};
  cef_string_t locales_dir_path = {};
  cef_string_t locale = {};
  cef_string_t cache_path = {};
  cef_string_t root_cache_path = {};
  neko_app_t* app = NULL;
  cef_app_t* cef_app = NULL;
  neko_cef_runtime_t* runtime = NULL;
  int initialized = 0;

  memset(&main_args, 0, sizeof(main_args));
  memset(&settings, 0, sizeof(settings));

  cef_api_hash(CEF_API_VERSION, 0);

  main_args.argc = argc;
  main_args.argv = argv;

  settings.size = sizeof(settings);
  settings.no_sandbox = options ? options->no_sandbox : 1;
  settings.multi_threaded_message_loop =
      options ? options->multi_threaded_message_loop : 0;
  settings.external_message_pump = options ? options->external_message_pump : 0;
  settings.windowless_rendering_enabled =
      options ? options->windowless_rendering_enabled : 1;
  settings.remote_debugging_port = options ? options->remote_debugging_port : 0;
  settings.disable_signal_handlers = 1;
  NEKO_CEF_LOG(
      "NEKO_CEF_C initialize begin argc=%d use_app=%d no_sandbox=%d multi_threaded=%d "
      "windowless=%d external_pump=%d subprocess=%s resources=%s locales=%s locale=%s\n",
      argc,
      options ? options->use_app : 0,
      settings.no_sandbox,
      settings.multi_threaded_message_loop,
      settings.windowless_rendering_enabled,
      settings.external_message_pump,
      options && options->browser_subprocess_path ? options->browser_subprocess_path : "<null>",
      options && options->resources_dir_path ? options->resources_dir_path : "<null>",
      options && options->locales_dir_path ? options->locales_dir_path : "<null>",
      options && options->locale ? options->locale : "<null>");

  neko_cef_set_cef_string(&browser_subprocess_path,
                          options ? options->browser_subprocess_path : NULL);
  neko_cef_set_cef_string(&resources_dir_path,
                          options ? options->resources_dir_path : NULL);
  neko_cef_set_cef_string(&locales_dir_path,
                          options ? options->locales_dir_path : NULL);
  neko_cef_set_cef_string(&locale, neko_cef_pick_string(options ? options->locale : NULL,
                                                        "en-US"));
  neko_cef_set_cef_string(&cache_path, options ? options->cache_path : NULL);
  neko_cef_set_cef_string(&root_cache_path, options ? options->root_cache_path : NULL);

  settings.browser_subprocess_path = browser_subprocess_path;
  settings.resources_dir_path = resources_dir_path;
  settings.locales_dir_path = locales_dir_path;
  settings.locale = locale;
  settings.cache_path = cache_path;
  settings.root_cache_path = root_cache_path;

  if (!options || options->use_app) {
    app = neko_app_create();
    if (!app) {
      neko_cef_write_error(error_message, error_message_capacity, "failed to allocate cef app");
      goto cleanup;
    }
    cef_app = &app->app;
    app->app.base.add_ref(&app->app.base);
  }

  if (!cef_initialize(&main_args, &settings, cef_app, NULL)) {
    NEKO_CEF_LOG("NEKO_CEF_C initialize failed\n");
    neko_cef_write_error(error_message, error_message_capacity, "cef_initialize returned 0");
    goto cleanup;
  }
  NEKO_CEF_LOG("NEKO_CEF_C initialize ok\n");
  initialized = 1;

  runtime = (neko_cef_runtime_t*)calloc(1, sizeof(neko_cef_runtime_t));
  if (!runtime) {
    neko_cef_write_error(error_message, error_message_capacity, "failed to allocate runtime");
    goto cleanup;
  }

  runtime->app = app;
  runtime->initialized = 1;
  runtime->message_loop_mode =
      settings.multi_threaded_message_loop ? NEKO_CEF_MESSAGE_LOOP_MULTI_THREADED
                                           : NEKO_CEF_MESSAGE_LOOP_EXTERNAL_PUMP;
  app = NULL;
  neko_cef_write_error(error_message, error_message_capacity, "");

cleanup:
  if (app) {
    app->app.base.release(&app->app.base);
  }

  if (!runtime && initialized) {
    cef_shutdown();
  }

  neko_cef_clear_cef_string(&browser_subprocess_path);
  neko_cef_clear_cef_string(&resources_dir_path);
  neko_cef_clear_cef_string(&locales_dir_path);
  neko_cef_clear_cef_string(&locale);
  neko_cef_clear_cef_string(&cache_path);
  neko_cef_clear_cef_string(&root_cache_path);

  return runtime;
}

void neko_cef_bridge_do_message_loop_work(neko_cef_runtime_t* runtime) {
  if (!runtime || !runtime->initialized) {
    return;
  }
  cef_do_message_loop_work();
}

int neko_cef_bridge_message_loop_mode(const neko_cef_runtime_t* runtime) {
  if (!runtime) {
    return NEKO_CEF_MESSAGE_LOOP_EXTERNAL_PUMP;
  }
  return runtime->message_loop_mode;
}

void neko_cef_bridge_shutdown(neko_cef_runtime_t* runtime) {
  if (!runtime) {
    return;
  }

  if (runtime->initialized) {
    cef_shutdown();
  }
  if (runtime->app) {
    runtime->app->app.base.release(&runtime->app->app.base);
    runtime->app = NULL;
  }
  free(runtime);
}

neko_cef_browser_t* neko_cef_bridge_create_browser(
    neko_cef_runtime_t* runtime,
    const neko_cef_browser_config_t* config,
    const neko_cef_browser_callbacks_t* callbacks,
    void* user_data,
    char* error_message,
    size_t error_message_capacity) {
  neko_cef_browser_t* bridge = NULL;
  cef_window_info_t window_info;
  cef_browser_settings_t browser_settings;
  cef_string_t url = {};
  cef_string_t window_name = {};
  int create_ok = 0;

  if (!runtime || !runtime->initialized) {
    neko_cef_write_error(error_message, error_message_capacity, "runtime is not initialized");
    return NULL;
  }

  if (!config || !config->url || config->url[0] == '\0') {
    neko_cef_write_error(error_message, error_message_capacity, "browser url is missing");
    return NULL;
  }

  if (config->width <= 0 || config->height <= 0) {
    neko_cef_write_error(error_message,
                         error_message_capacity,
                         "browser width and height must be positive");
    return NULL;
  }

  if (config->frame_rate <= 0) {
    neko_cef_write_error(error_message, error_message_capacity, "browser frame_rate must be positive");
    return NULL;
  }

  bridge = neko_cef_browser_create(runtime, callbacks, user_data);
  if (!bridge) {
    neko_cef_write_error(error_message, error_message_capacity, "failed to allocate browser bridge");
    return NULL;
  }

  bridge->width = config->width;
  bridge->height = config->height;
  bridge->transparent_painting = config->transparent_painting ? 1 : 0;
  NEKO_CEF_LOG("NEKO_CEF_C create_browser begin url=%s size=%dx%d fps=%d transparent=%d\n",
               config->url,
               config->width,
               config->height,
               config->frame_rate,
               config->transparent_painting);

  memset(&window_info, 0, sizeof(window_info));
  memset(&browser_settings, 0, sizeof(browser_settings));

  window_info.size = sizeof(window_info);
  neko_cef_set_cef_string(&window_name,
                          neko_cef_pick_string(config->window_name, "neko-cef-bridge"));
  window_info.window_name = window_name;
  window_info.bounds.x = 0;
  window_info.bounds.y = 0;
  window_info.bounds.width = config->width;
  window_info.bounds.height = config->height;
  window_info.windowless_rendering_enabled = 1;
  window_info.shared_texture_enabled = 0;
  window_info.external_begin_frame_enabled = 0;

  browser_settings.size = sizeof(browser_settings);
  browser_settings.windowless_frame_rate = config->frame_rate;
  browser_settings.background_color =
      config->transparent_painting ? 0x00000000 : 0xFFFFFFFF;

  neko_cef_set_cef_string(&url, config->url);

  bridge->client.base.add_ref(&bridge->client.base);
  create_ok = cef_browser_host_create_browser(
      &window_info,
      &bridge->client,
      &url,
      &browser_settings,
      NULL,
      NULL);
  NEKO_CEF_LOG("NEKO_CEF_C create_browser returned %d\n", create_ok);
  if (!create_ok) {
    bridge->client.base.release(&bridge->client.base);
    neko_cef_write_error(error_message,
                         error_message_capacity,
                         "cef_browser_host_create_browser returned 0");
    neko_cef_clear_cef_string(&url);
    neko_cef_clear_cef_string(&window_name);
    neko_cef_browser_release_ref(bridge);
    return NULL;
  }

  neko_cef_write_error(error_message, error_message_capacity, "");
  neko_cef_clear_cef_string(&url);
  neko_cef_clear_cef_string(&window_name);
  return bridge;
}

void neko_cef_bridge_browser_close(neko_cef_browser_t* bridge) {
  if (!bridge || !bridge->host || atomic_load(&bridge->closed)) {
    return;
  }
  if (bridge->host->close_browser) {
    bridge->host->close_browser(bridge->host, 1);
  }
}

void neko_cef_bridge_browser_release(neko_cef_browser_t* bridge) {
  neko_cef_browser_release_ref(bridge);
}

int neko_cef_bridge_browser_is_ready(const neko_cef_browser_t* bridge) {
  if (!bridge) {
    return 0;
  }
  return bridge->host != NULL && bridge->browser != NULL && !atomic_load(&bridge->closed);
}

void neko_cef_bridge_browser_set_focus(neko_cef_browser_t* bridge, int focused) {
  if (!bridge || !bridge->host || !bridge->host->set_focus) {
    return;
  }
  bridge->host->set_focus(bridge->host, focused ? 1 : 0);
}

void neko_cef_bridge_browser_notify_resized(neko_cef_browser_t* bridge,
                                            int width,
                                            int height) {
  if (!bridge) {
    return;
  }
  if (width > 0) {
    bridge->width = width;
  }
  if (height > 0) {
    bridge->height = height;
  }
  if (bridge->host && bridge->host->was_resized) {
    bridge->host->was_resized(bridge->host);
  }
}

static cef_mouse_button_type_t neko_cef_map_mouse_button(int button) {
  switch (button) {
    case NEKO_CEF_MOUSE_BUTTON_MIDDLE:
      return MBT_MIDDLE;
    case NEKO_CEF_MOUSE_BUTTON_RIGHT:
      return MBT_RIGHT;
    case NEKO_CEF_MOUSE_BUTTON_LEFT:
    default:
      return MBT_LEFT;
  }
}

static cef_key_event_type_t neko_cef_map_key_event(int kind) {
  switch (kind) {
    case NEKO_CEF_KEY_EVENT_KEY_UP:
      return KEYEVENT_KEYUP;
    case NEKO_CEF_KEY_EVENT_CHAR:
      return KEYEVENT_CHAR;
    case NEKO_CEF_KEY_EVENT_RAW_KEY_DOWN:
    default:
      return KEYEVENT_RAWKEYDOWN;
  }
}

void neko_cef_bridge_browser_send_mouse_move(neko_cef_browser_t* bridge,
                                             int x,
                                             int y,
                                             int mouse_leave,
                                             uint32_t modifiers) {
  cef_mouse_event_t event;
  if (!bridge || !bridge->host || !bridge->host->send_mouse_move_event) {
    return;
  }
  memset(&event, 0, sizeof(event));
  event.x = x;
  event.y = y;
  event.modifiers = modifiers;
  bridge->host->send_mouse_move_event(bridge->host, &event, mouse_leave ? 1 : 0);
}

void neko_cef_bridge_browser_send_mouse_click(neko_cef_browser_t* bridge,
                                              int x,
                                              int y,
                                              int button,
                                              int mouse_up,
                                              int click_count,
                                              uint32_t modifiers) {
  cef_mouse_event_t event;
  if (!bridge || !bridge->host || !bridge->host->send_mouse_click_event) {
    return;
  }
  memset(&event, 0, sizeof(event));
  event.x = x;
  event.y = y;
  event.modifiers = modifiers;
  bridge->host->send_mouse_click_event(bridge->host,
                                       &event,
                                       neko_cef_map_mouse_button(button),
                                       mouse_up ? 1 : 0,
                                       click_count);
}

void neko_cef_bridge_browser_send_mouse_wheel(neko_cef_browser_t* bridge,
                                              int x,
                                              int y,
                                              int delta_x,
                                              int delta_y,
                                              uint32_t modifiers) {
  cef_mouse_event_t event;
  if (!bridge || !bridge->host || !bridge->host->send_mouse_wheel_event) {
    return;
  }
  memset(&event, 0, sizeof(event));
  event.x = x;
  event.y = y;
  event.modifiers = modifiers;
  bridge->host->send_mouse_wheel_event(bridge->host, &event, delta_x, delta_y);
}

void neko_cef_bridge_browser_send_key_event(neko_cef_browser_t* bridge,
                                            int kind,
                                            int windows_key_code,
                                            int native_key_code,
                                            uint32_t modifiers,
                                            uint16_t character,
                                            uint16_t unmodified_character) {
  cef_key_event_t event;
  if (!bridge || !bridge->host || !bridge->host->send_key_event) {
    return;
  }
  memset(&event, 0, sizeof(event));
  event.size = sizeof(event);
  event.type = neko_cef_map_key_event(kind);
  event.modifiers = modifiers;
  event.windows_key_code = windows_key_code;
  event.native_key_code = native_key_code;
  event.character = character;
  event.unmodified_character = unmodified_character;
  bridge->host->send_key_event(bridge->host, &event);
}

void neko_cef_bridge_browser_execute_javascript(neko_cef_browser_t* bridge,
                                                const char* code,
                                                const char* script_url,
                                                int start_line) {
  cef_frame_t* frame;
  cef_string_t cef_code = {};
  cef_string_t cef_script_url = {};

  if (!bridge || !bridge->browser || !bridge->browser->get_main_frame || !code || !code[0]) {
    return;
  }

  frame = bridge->browser->get_main_frame(bridge->browser);
  if (!frame || !frame->execute_java_script) {
    return;
  }

  neko_cef_set_cef_string(&cef_code, code);
  neko_cef_set_cef_string(
      &cef_script_url, neko_cef_pick_string(script_url, "neko://standalone-helper"));
  frame->execute_java_script(frame, &cef_code, &cef_script_url, start_line);
  neko_cef_clear_cef_string(&cef_code);
  neko_cef_clear_cef_string(&cef_script_url);
}

static int neko_cef_capi_run_impl(int argc, char** argv, int use_app) {
  char cwd[4096];
  char root_cache_path[4096];
  char locales_path[4096];
  neko_cef_runtime_settings_t settings;
  neko_cef_runtime_t* runtime;
  char error[512];

  memset(&settings, 0, sizeof(settings));
  settings.no_sandbox = 1;
  settings.windowless_rendering_enabled = 1;
  settings.use_app = use_app;

  if (getcwd(cwd, sizeof(cwd)) != NULL) {
    if (neko_cef_join_path(root_cache_path, sizeof(root_cache_path), cwd, "/.cef-cache-probe") &&
        neko_cef_join_path(locales_path, sizeof(locales_path), cwd, "/locales")) {
      settings.resources_dir_path = cwd;
      settings.locales_dir_path = locales_path;
      settings.locale = "en-US";
      settings.cache_path = root_cache_path;
      settings.root_cache_path = root_cache_path;
    }
  }

  runtime = neko_cef_bridge_initialize(argc, argv, &settings, error, sizeof(error));
  if (!runtime) {
    return 92;
  }
  neko_cef_bridge_shutdown(runtime);
  return 0;
}

int neko_cef_capi_probe_run(int argc, char** argv) {
  return neko_cef_capi_run_impl(argc, argv, 1);
}

int neko_cef_capi_probe_run_null_app(int argc, char** argv) {
  return neko_cef_capi_run_impl(argc, argv, 0);
}
