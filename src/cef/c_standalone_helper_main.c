#define _DEFAULT_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#include "include/capi/cef_app_capi.h"

#define NEKO_COMMAND_BUFFER_CAPACITY 131072

typedef struct _neko_cef_runtime_t neko_cef_runtime_t;
typedef struct _neko_cef_browser_t neko_cef_browser_t;

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
  void (*on_after_created)(void* user_data);
  void (*on_before_close)(void* user_data);
  void (*on_loading_state_change)(void* user_data,
                                  int is_loading,
                                  int can_go_back,
                                  int can_go_forward);
  void (*on_load_start)(void* user_data, int transition_type);
  void (*on_load_end)(void* user_data, int http_status_code);
  void (*on_load_error)(void* user_data,
                        int error_code,
                        const char* error_text,
                        const char* failed_url);
  void (*on_console)(void* user_data,
                     int level,
                     const char* source,
                     int line,
                     const char* message);
  void (*on_paint)(void* user_data,
                   int element_type,
                   const void* buffer,
                   int width,
                   int height);
} neko_cef_browser_callbacks_t;

typedef struct _neko_cef_standalone_state_t {
  int frame_count;
  int frame_dump_log_emitted;
  char frame_dump_path[1024];
  void* shared_frame_map;
  size_t shared_frame_map_len;
  int shared_frame_fd;
  neko_cef_browser_t* browser;
} neko_cef_standalone_state_t;

typedef struct _neko_shared_frame_header_t {
  uint32_t magic;
  uint32_t version;
  uint32_t slot_count;
  uint32_t latest_slot;
  uint32_t reserved0;
  uint32_t reserved1;
  uint32_t reserved2;
  uint32_t reserved3;
} neko_shared_frame_root_header_t;

typedef struct _neko_shared_frame_slot_header_t {
  uint32_t seq;
  uint32_t frame;
  uint32_t width;
  uint32_t height;
  uint32_t stride;
  uint32_t data_len;
  uint32_t reserved0;
  uint32_t reserved1;
} neko_shared_frame_slot_header_t;

#define NEKO_SHARED_FRAME_MAGIC 0x4E4B4642u
#define NEKO_SHARED_FRAME_VERSION 1u
#define NEKO_SHARED_FRAME_SLOT_COUNT 3u

extern int neko_cef_bridge_execute_process(int argc, char** argv, int use_app);
extern neko_cef_runtime_t* neko_cef_bridge_initialize(
    int argc,
    char** argv,
    const neko_cef_runtime_settings_t* options,
    char* error_message,
    size_t error_message_capacity);
extern neko_cef_browser_t* neko_cef_bridge_create_browser(
    neko_cef_runtime_t* runtime,
    const neko_cef_browser_config_t* config,
    const neko_cef_browser_callbacks_t* callbacks,
    void* user_data,
    char* error_message,
    size_t error_message_capacity);
extern void neko_cef_bridge_browser_close(neko_cef_browser_t* browser);
extern void neko_cef_bridge_browser_release(neko_cef_browser_t* browser);
extern void neko_cef_bridge_browser_set_focus(neko_cef_browser_t* browser, int focused);
extern void neko_cef_bridge_browser_send_mouse_move(neko_cef_browser_t* browser,
                                                    int x,
                                                    int y,
                                                    int mouse_leave,
                                                    unsigned int modifiers);
extern void neko_cef_bridge_browser_send_mouse_click(neko_cef_browser_t* browser,
                                                     int x,
                                                     int y,
                                                     int button,
                                                     int mouse_up,
                                                     int click_count,
                                                     unsigned int modifiers);
extern void neko_cef_bridge_browser_send_mouse_wheel(neko_cef_browser_t* browser,
                                                     int x,
                                                     int y,
                                                     int delta_x,
                                                     int delta_y,
                                                     unsigned int modifiers);
extern void neko_cef_bridge_browser_send_key_event(neko_cef_browser_t* browser,
                                                   int kind,
                                                   int windows_key_code,
                                                   int native_key_code,
                                                   unsigned int modifiers,
                                                   unsigned short character,
                                                   unsigned short unmodified_character);
extern void neko_cef_bridge_browser_execute_javascript(neko_cef_browser_t* browser,
                                                       const char* code,
                                                       const char* script_url,
                                                       int start_line);
extern void neko_cef_bridge_shutdown(neko_cef_runtime_t* runtime);

static const char* env_or_null(const char* name) {
  const char* value = getenv(name);
  if (!value || !value[0]) {
    return NULL;
  }
  return value;
}

static int env_flag_enabled(const char* name) {
  const char* value = env_or_null(name);
  if (!value) {
    return 0;
  }
  return strcmp(value, "1") == 0 || strcmp(value, "true") == 0 || strcmp(value, "TRUE") == 0 ||
         strcmp(value, "yes") == 0 || strcmp(value, "YES") == 0 || strcmp(value, "on") == 0 ||
         strcmp(value, "ON") == 0;
}

static int env_int_or_default(const char* name, int default_value) {
  const char* value = env_or_null(name);
  char* end = NULL;
  long parsed;
  if (!value) {
    return default_value;
  }
  parsed = strtol(value, &end, 10);
  if (end == value || (end && *end != '\0') || parsed <= 0 || parsed > 32768) {
    return default_value;
  }
  return (int)parsed;
}

static size_t env_size_or_zero(const char* name) {
  const char* value = env_or_null(name);
  char* end = NULL;
  unsigned long long parsed;
  if (!value) {
    return 0;
  }
  parsed = strtoull(value, &end, 10);
  if (end == value || (end && *end != '\0')) {
    return 0;
  }
  return (size_t)parsed;
}

static void emit_event(const char* event, const char* details) {
  if (!event) {
    return;
  }
  if (!details) {
    details = "";
  }
  printf("NEKO_CEF_STANDALONE_EVENT event=%s%s%s\n",
         event,
         details[0] ? " " : "",
         details);
  fflush(stdout);
}

static void sleep_ms(long ms) {
  struct timespec req;
  req.tv_sec = ms / 1000;
  req.tv_nsec = (ms % 1000) * 1000000L;
  nanosleep(&req, NULL);
}

static int configure_nonblocking_stdin(void) {
  int flags = fcntl(STDIN_FILENO, F_GETFL, 0);
  if (flags < 0) {
    fprintf(stderr, "NEKO_CEF_STANDALONE failed to get stdin flags: %s\n", strerror(errno));
    fflush(stderr);
    return 0;
  }
  if (fcntl(STDIN_FILENO, F_SETFL, flags | O_NONBLOCK) < 0) {
    fprintf(stderr, "NEKO_CEF_STANDALONE failed to set nonblocking stdin: %s\n", strerror(errno));
    fflush(stderr);
    return 0;
  }
  return 1;
}

static void trim_newlines(char* line) {
  size_t len;
  if (!line) {
    return;
  }
  len = strlen(line);
  while (len > 0 && (line[len - 1] == '\n' || line[len - 1] == '\r')) {
    line[len - 1] = '\0';
    len -= 1;
  }
}

static int base64_value(char ch) {
  if (ch >= 'A' && ch <= 'Z') return ch - 'A';
  if (ch >= 'a' && ch <= 'z') return ch - 'a' + 26;
  if (ch >= '0' && ch <= '9') return ch - '0' + 52;
  if (ch == '+') return 62;
  if (ch == '/') return 63;
  if (ch == '=') return -2;
  return -1;
}

static char* base64_decode_alloc(const char* input, size_t* out_len) {
  size_t len;
  size_t alloc_len;
  char* output;
  size_t out = 0;
  int vals[4];
  size_t i;

  if (out_len) {
    *out_len = 0;
  }
  if (!input) {
    return NULL;
  }

  len = strlen(input);
  alloc_len = (len / 4 + 1) * 3 + 1;
  output = (char*)malloc(alloc_len);
  if (!output) {
    return NULL;
  }

  for (i = 0; i < len;) {
    int count = 0;
    while (i < len && count < 4) {
      int value = base64_value(input[i++]);
      if (value == -1) {
        continue;
      }
      vals[count++] = value;
    }
    if (count == 0) {
      break;
    }
    if (count < 4) {
      free(output);
      return NULL;
    }

    output[out++] = (char)((vals[0] << 2) | ((vals[1] & 0x30) >> 4));
    if (vals[2] != -2) {
      output[out++] = (char)(((vals[1] & 0x0F) << 4) | ((vals[2] & 0x3C) >> 2));
    }
    if (vals[3] != -2 && vals[2] != -2) {
      output[out++] = (char)(((vals[2] & 0x03) << 6) | vals[3]);
    }
    if (vals[2] == -2 || vals[3] == -2) {
      break;
    }
  }

  output[out] = '\0';
  if (out_len) {
    *out_len = out;
  }
  return output;
}

static int process_command_line(neko_cef_browser_t* browser, const char* line) {
  int focused;
  int x;
  int y;
  int leave;
  unsigned int modifiers;
  int button;
  int up;
  int clicks;
  int dx;
  int dy;
  int kind;
  int win;
  int native_code;
  unsigned int key_modifiers;
  unsigned int ch;
  unsigned int unmod;

  if (!browser || !line || !line[0]) {
    return 0;
  }

  if (strcmp(line, "shutdown") == 0) {
    return 1;
  }

  if (strncmp(line, "eval_b64 script_b64=", 20) == 0) {
    size_t script_len = 0;
    char* script = base64_decode_alloc(line + 20, &script_len);
    if (!script || script_len == 0) {
      fprintf(stderr, "NEKO_CEF_STANDALONE failed to decode eval_b64 payload\n");
      fflush(stderr);
      free(script);
      return 0;
    }
    neko_cef_bridge_browser_execute_javascript(browser, script, "neko://stdin-eval", 1);
    free(script);
    return 0;
  }

  if (sscanf(line, "focus focused=%d", &focused) == 1) {
    neko_cef_bridge_browser_set_focus(browser, focused ? 1 : 0);
    return 0;
  }

  if (sscanf(line, "mouse_move x=%d y=%d leave=%d modifiers=%u", &x, &y, &leave, &modifiers) == 4) {
    neko_cef_bridge_browser_send_mouse_move(browser, x, y, leave, modifiers);
    return 0;
  }

  if (sscanf(line,
             "mouse_click x=%d y=%d button=%d up=%d clicks=%d modifiers=%u",
             &x,
             &y,
             &button,
             &up,
             &clicks,
             &modifiers) == 6) {
    neko_cef_bridge_browser_send_mouse_click(browser, x, y, button, up, clicks, modifiers);
    return 0;
  }

  if (sscanf(line, "mouse_wheel x=%d y=%d dx=%d dy=%d modifiers=%u", &x, &y, &dx, &dy, &modifiers) ==
      5) {
    neko_cef_bridge_browser_send_mouse_wheel(browser, x, y, dx, dy, modifiers);
    return 0;
  }

  if (sscanf(line,
             "key kind=%d win=%d native=%d modifiers=%u char=%u unmod=%u",
             &kind,
             &win,
             &native_code,
             &key_modifiers,
             &ch,
             &unmod) == 6) {
    neko_cef_bridge_browser_send_key_event(browser,
                                           kind,
                                           win,
                                           native_code,
                                           key_modifiers,
                                           (unsigned short)ch,
                                           (unsigned short)unmod);
    return 0;
  }

  fprintf(stderr, "NEKO_CEF_STANDALONE unknown stdin command: %s\n", line);
  fflush(stderr);
  return 0;
}

static int process_stdin_commands(neko_cef_browser_t* browser, char* command_buffer, size_t* used) {
  ssize_t bytes_read;
  char* newline;

  if (!browser || !command_buffer || !used) {
    return 0;
  }

  while ((bytes_read = read(STDIN_FILENO,
                            command_buffer + *used,
                            (NEKO_COMMAND_BUFFER_CAPACITY - 1) - *used)) > 0) {
    *used += (size_t)bytes_read;
    command_buffer[*used] = '\0';
    while ((newline = strchr(command_buffer, '\n')) != NULL) {
      size_t consumed;
      *newline = '\0';
      trim_newlines(command_buffer);
      if (process_command_line(browser, command_buffer)) {
        return 1;
      }
      consumed = (size_t)(newline - command_buffer + 1);
      memmove(command_buffer, command_buffer + consumed, *used - consumed);
      *used -= consumed;
      command_buffer[*used] = '\0';
    }
    if (*used >= (NEKO_COMMAND_BUFFER_CAPACITY - 1)) {
      fprintf(stderr, "NEKO_CEF_STANDALONE stdin command buffer overflow, resetting\n");
      fflush(stderr);
      *used = 0;
      command_buffer[0] = '\0';
    }
  }

  if (bytes_read < 0 && errno != EAGAIN && errno != EWOULDBLOCK) {
    fprintf(stderr, "NEKO_CEF_STANDALONE stdin read failed: %s\n", strerror(errno));
    fflush(stderr);
  }

  return 0;
}

static void on_after_created(void* user_data) {
  neko_cef_standalone_state_t* state = (neko_cef_standalone_state_t*)user_data;
  emit_event("browser_created", "");
  if (state && !state->browser) {
    /* browser pointer is assigned from main after create_browser returns */
  }
  fprintf(stderr, "NEKO_CEF_STANDALONE browser_created\n");
  fflush(stderr);
}

static void on_before_close(void* user_data) {
  (void)user_data;
  emit_event("browser_before_close", "");
  fprintf(stderr, "NEKO_CEF_STANDALONE browser_before_close\n");
  fflush(stderr);
}

static void on_load_start(void* user_data, int transition_type) {
  char details[128];
  (void)user_data;
  snprintf(details, sizeof(details), "transition_type=%d", transition_type);
  emit_event("load_start", details);
  fprintf(stderr, "NEKO_CEF_STANDALONE load_start transition_type=%d\n", transition_type);
  fflush(stderr);
}

static void on_load_end(void* user_data, int http_status_code) {
  char details[128];
  neko_cef_standalone_state_t* state = (neko_cef_standalone_state_t*)user_data;
  snprintf(details, sizeof(details), "http_status_code=%d", http_status_code);
  emit_event("load_end", details);
  fprintf(stderr, "NEKO_CEF_STANDALONE load_end http_status_code=%d\n", http_status_code);
  fflush(stderr);
  if (state && state->browser && http_status_code >= 200 && http_status_code < 400) {
    static const char* k_region_script =
        "(function(){"
        "if(window.__NEKO_INPUT_REGION_BRIDGE_INSTALLED__)return;"
        "window.__NEKO_INPUT_REGION_BRIDGE_INSTALLED__=true;"
        "const transparentBg=new URLSearchParams(location.search).get('neko_transparent_bg')==='1';"
        "if(transparentBg){"
        "try{"
        "const style=document.createElement('style');"
        "style.id='neko-wayland-transparent-style';"
        "style.textContent='html,body,#live2d-container,#vrm-container{background:transparent!important;} body{overflow:hidden!important;}';"
        "if(!document.getElementById(style.id))document.documentElement.appendChild(style);"
        "}catch(_){ }"
        "}"
        "const selectors=['#live2d-floating-buttons','.live2d-floating-btn','#live2d-lock-icon',"
        "'#vrm-floating-buttons','#vrm-lock-icon','#vrm-lock-icon-hidden',"
        "'#live2d-return-button-container','#chat-container','#chat-header','#toggle-chat-btn',"
        "'#chatContainer','#text-input-area','#textInputBox','#button-group','#screenshots-list',"
        "'#screenshots-header','.chat-resize-handle','.modal-dialog','.modal-overlay',"
        "'.live2d-popup','.vrm-popup','[id^=\"live2d-popup-\"]','[id^=\"vrm-popup-\"]',"
        "'[data-neko-sidepanel]','[data-neko-sidepanel-owner]','[data-neko-interactive]',"
        "'.driver-overlay','.driver-highlight','.driver-popover','.driver-popover-footer',"
        "'.driver-next','.driver-prev','.driver-close-btn','.driver-popover-title',"
        "'#neko-tutorial-skip-btn','#agent-task-hud','#agent-task-list','#live2d-agent-status',"
        "'.task-card','.task-card-cancel','.neko-systray-menu','.neko-systray-menu__panel',"
        "'.neko-systray-location','[id*=\"tutorial-target\"]','[class*=\"tutorial\"]',"
        "'[class*=\"popover\"]','[class*=\"sidepanel\"]','[class*=\"sidebar\"]'];"
        "const interactiveLeafSelector='button,input,textarea,select,summary,a[href],[role=\"button\"],[role=\"dialog\"],[role=\"menu\"],[role=\"menuitem\"],[role=\"tab\"],[role=\"switch\"],[role=\"checkbox\"],[role=\"radio\"],[role=\"slider\"],[role=\"textbox\"],[role=\"tooltip\"],[role=\"option\"],[role=\"listbox\"],[contenteditable=\"true\"],[tabindex],[aria-haspopup=\"true\"],[data-neko-interactive],[data-action],[onclick]';"
        "let last='';"
        "let lastDrag='';"
        "let lastNoDrag='';"
        "let captureCanvas=null;"
        "let scheduled=false;"
        "let timer=0;"
        "function interactiveTag(el){"
        "const text=[el.id||'',el.className||'',el.getAttribute('role')||'',"
        "el.getAttribute('data-action')||'',(el.parentElement&&el.parentElement.id)||'',"
        "(el.parentElement&&el.parentElement.className)||''].join(' ').toLowerCase();"
        "return text.includes('live2d')||text.includes('vrm')||text.includes('l2d')||"
        "text.includes('dialog')||text.includes('popup')||text.includes('popover')||"
        "text.includes('bubble')||text.includes('button')||text.includes('chat')||"
        "text.includes('input')||text.includes('tutorial')||text.includes('guide')||"
        "text.includes('driver')||text.includes('highlight')||text.includes('overlay')||"
        "text.includes('skip')||text.includes('panel')||text.includes('sidebar')||"
        "text.includes('drawer')||text.includes('menu')||text.includes('hud')||"
        "text.includes('agent')||text.includes('task');"
        "}"
        "function visibleAndEnabled(el){"
        "let node=el;"
        "while(node&&node.nodeType===1){"
        "const style=getComputedStyle(node);"
        "if(style.display==='none'||style.visibility==='hidden'||Number(style.opacity)===0)return false;"
        "node=node.parentElement;"
        "}"
        "return true;"
        "}"
        "function acceptsPointer(el){"
        "let node=el;"
        "while(node&&node.nodeType===1){"
        "const style=getComputedStyle(node);"
        "if(style.pointerEvents==='none')return false;"
        "node=node.parentElement;"
        "}"
        "return true;"
        "}"
        "function appRegion(el){"
        "const style=getComputedStyle(el);"
        "return (style.getPropertyValue('-webkit-app-region')||style.getPropertyValue('app-region')||style.webkitAppRegion||'').trim().toLowerCase();"
        "}"
        "function numericZIndex(value){"
        "const z=Number.parseInt(value||'0',10);"
        "return Number.isFinite(z)?z:0;"
        "}"
        "function looksClickable(el){"
        "if(!el)return false;"
        "const style=getComputedStyle(el);"
        "const cursor=(style.cursor||'').toLowerCase();"
        "return cursor==='pointer'||cursor==='text'||cursor==='move'||cursor==='grab'||cursor==='grabbing'||typeof el.onclick==='function'||el.hasAttribute('onclick')||el.hasAttribute('data-action');"
        "}"
        "function hasInteractiveDescendant(el){"
        "if(!el||typeof el.matches!=='function')return false;"
        "if(el.matches(interactiveLeafSelector))return true;"
        "return !!el.querySelector(interactiveLeafSelector);"
        "}"
        "function isInteractiveLeaf(el){"
        "if(!el||typeof el.matches!=='function')return false;"
        "if(el.matches(interactiveLeafSelector))return true;"
        "return looksClickable(el)&&interactiveTag(el);"
        "}"
        "function hasVisualSurface(style){"
        "const bg=(style.backgroundColor||'').toLowerCase();"
        "const borderW=(style.borderTopWidth||'0')!=='0px'||(style.borderRightWidth||'0')!=='0px'||(style.borderBottomWidth||'0')!=='0px'||(style.borderLeftWidth||'0')!=='0px';"
        "return (bg&&bg!=='transparent'&&bg!=='rgba(0, 0, 0, 0)'&&bg!=='rgb(0, 0, 0, 0)')||style.boxShadow!=='none'||style.backdropFilter!=='none'||style.filter!=='none'||borderW;"
        "}"
        "function looksLikeInteractiveOverlay(el){"
        "if(!el)return false;"
        "const style=getComputedStyle(el);"
        "const pos=(style.position||'').toLowerCase();"
        "const z=numericZIndex(style.zIndex);"
        "const r=el.getBoundingClientRect();"
        "const huge=r.width>=window.innerWidth*0.82&&r.height>=window.innerHeight*0.55;"
        "const almostFull=r.width>=window.innerWidth*0.94&&r.height>=window.innerHeight*0.94;"
        "const tagged=interactiveTag(el)||hasInteractiveDescendant(el);"
        "if((pos==='fixed'||pos==='absolute')&&z>=9999&&tagged)return true;"
        "if(almostFull&&z>=9000&&tagged)return true;"
        "if(huge&&tagged&&el.querySelector&&el.querySelector('button,.driver-popover,[data-neko-sidepanel],#neko-tutorial-skip-btn,#agent-task-hud,.task-card'))return true;"
        "return false;"
        "}"
        "function shouldPromoteInteractiveAncestor(el){"
        "if(!el||el===document.body||el===document.documentElement)return false;"
        "if(!visibleAndEnabled(el)||!acceptsPointer(el))return false;"
        "const r=el.getBoundingClientRect();"
        "if(r.width<8||r.height<8)return false;"
        "const style=getComputedStyle(el);"
        "const pos=(style.position||'').toLowerCase();"
        "const z=numericZIndex(style.zIndex);"
        "const role=(el.getAttribute('role')||'').toLowerCase();"
        "const tagged=interactiveTag(el);"
        "const descendantInteractive=hasInteractiveDescendant(el);"
        "const floating=pos==='fixed'||pos==='absolute'||pos==='sticky';"
        "const surfaced=hasVisualSurface(style);"
        "const huge=r.width>=window.innerWidth*0.94&&r.height>=window.innerHeight*0.94;"
        "const semantic=role==='dialog'||role==='menu'||role==='menuitem'||role==='tooltip'||role==='listbox'||role==='group'||role==='region';"
        "if(looksLikeInteractiveOverlay(el))return true;"
        "if(huge)return false;"
        "if(semantic&&descendantInteractive)return true;"
        "if(floating&&surfaced&&descendantInteractive)return true;"
        "if(z>=100&&surfaced&&(tagged||descendantInteractive))return true;"
        "if(tagged&&descendantInteractive&&(floating||surfaced))return true;"
        "return false;"
        "}"
        "function promoteInteractiveAncestors(rects,el,seen){"
        "let depth=0;"
        "for(let node=el.parentElement;node&&node!==document.body&&node!==document.documentElement;node=node.parentElement){"
        "if(depth++>6)break;"
        "if(!shouldPromoteInteractiveAncestor(node))continue;"
        "if(!seen.has(node)){seen.add(node);addRect(rects,node.getBoundingClientRect());}"
        "const r=node.getBoundingClientRect();"
        "if(r.width>=window.innerWidth*0.9&&r.height>=window.innerHeight*0.9)break;"
        "}"
        "}"
        "function addRect(rects,r){"
        "const left=Math.max(0,Math.round(r.left));"
        "const top=Math.max(0,Math.round(r.top));"
        "const right=Math.min(window.innerWidth,Math.round(r.right));"
        "const bottom=Math.min(window.innerHeight,Math.round(r.bottom));"
        "const width=right-left;"
        "const height=bottom-top;"
        "if(width<2||height<2)return;"
        "rects.push({x:left,y:top,width:width,height:height});"
        "}"
        "function addPaddedRect(rects,r,padX,padY){"
        "addRect(rects,{left:r.left-padX,top:r.top-padY,right:r.right+padX,bottom:r.bottom+padY});"
        "}"
        "function rectContains(outer,inner){"
        "return inner.x>=outer.x&&inner.y>=outer.y&&inner.x+inner.width<=outer.x+outer.width&&inner.y+inner.height<=outer.y+outer.height;"
        "}"
        "function normalizeRects(rects,maxRects){"
        "const normalized=[];"
        "for(const rect of rects){"
        "let skip=false;"
        "for(let i=normalized.length-1;i>=0;i--){"
        "const other=normalized[i];"
        "if(rect.x===other.x&&rect.y===other.y&&rect.width===other.width&&rect.height===other.height){skip=true;break;}"
        "if(rectContains(other,rect)){skip=true;break;}"
        "if(rectContains(rect,other)){normalized.splice(i,1);}"
        "}"
        "if(!skip)normalized.push(rect);"
        "if(maxRects&&normalized.length>=maxRects)break;"
        "}"
        "return normalized;"
        "}"
        "function addApproxRects(rects,left,top,right,bottom){"
        "const width=Math.max(1,right-left);"
        "const height=Math.max(1,bottom-top);"
        "const centerX=left+width/2;"
        "const aspect=width/height;"
        "const slim=aspect<0.42;"
        "const crownW=width*(slim?0.09:0.11);"
        "const headW=width*(slim?0.17:0.20);"
        "const faceW=width*(slim?0.11:0.13);"
        "const hairSideW=width*(slim?0.045:0.06);"
        "const shoulderW=width*(slim?0.33:0.39);"
        "const waistW=width*(slim?0.26:0.30);"
        "const legW=width*(slim?0.10:0.12);"
        "const legGap=width*(slim?0.08:0.10);"
        "const parts=["
        "{cx:centerX,y:top+height*0.025,w:crownW,h:height*0.045},"
        "{cx:centerX-headW*0.56,y:top+height*0.07,w:hairSideW,h:height*0.07},"
        "{cx:centerX+headW*0.56,y:top+height*0.07,w:hairSideW,h:height*0.07},"
        "{cx:centerX-headW*0.70,y:top+height*0.13,w:hairSideW,h:height*0.09},"
        "{cx:centerX+headW*0.70,y:top+height*0.13,w:hairSideW,h:height*0.09},"
        "{cx:centerX-headW*0.48,y:top+height*0.20,w:hairSideW,h:height*0.07},"
        "{cx:centerX+headW*0.48,y:top+height*0.20,w:hairSideW,h:height*0.07},"
        "{cx:centerX,y:top+height*0.085,w:headW,h:height*0.075},"
        "{cx:centerX,y:top+height*0.155,w:faceW,h:height*0.085},"
        "{cx:centerX,y:top+height*0.24,w:faceW*0.55,h:height*0.04},"
        "{cx:centerX,y:top+height*0.26,w:shoulderW,h:height*0.17},"
        "{cx:centerX,y:top+height*0.45,w:waistW,h:height*0.17},"
        "{cx:centerX-(legGap/2+legW/2),y:top+height*0.64,w:legW,h:height*0.18},"
        "{cx:centerX+(legGap/2+legW/2),y:top+height*0.64,w:legW,h:height*0.18}"
        "];"
        "for(const part of parts){"
        "addRect(rects,{left:part.cx-part.w/2,top:part.y,right:part.cx+part.w/2,bottom:part.y+part.h});"
        "}"
        "}"
        "function getCaptureContext(width,height){"
        "if(!captureCanvas){"
        "captureCanvas=document.createElement('canvas');"
        "captureCanvas.id='neko-wayland-live2d-mask-canvas';"
        "}"
        "if(captureCanvas.width!==width)captureCanvas.width=width;"
        "if(captureCanvas.height!==height)captureCanvas.height=height;"
        "return captureCanvas.getContext('2d',{willReadFrequently:true});"
        "}"
        "function addLive2DModelRect(rects){"
        "try{"
        "const manager=window.live2dManager;"
        "if(!manager)return;"
        "if(manager.isLocked)return;"
        "const model=typeof manager.getCurrentModel==='function'?manager.getCurrentModel():manager.currentModel;"
        "if(!model||typeof model.getBounds!=='function')return;"
        "const canvas=document.getElementById('live2d-canvas');"
        "if(!canvas||!canvas.width||!canvas.height)return;"
        "const bounds=model.getBounds();"
        "if(!bounds)return;"
        "const left=Number(bounds.left);"
        "const top=Number(bounds.top);"
        "const right=Number(bounds.right);"
        "const bottom=Number(bounds.bottom);"
        "const width=right-left;"
        "const height=bottom-top;"
        "if(!Number.isFinite(width)||!Number.isFinite(height)||width<8||height<8)return;"
        "const clipLeft=Math.max(0,Math.floor(left));"
        "const clipTop=Math.max(0,Math.floor(top));"
        "const clipRight=Math.min(window.innerWidth,Math.ceil(right));"
        "const clipBottom=Math.min(window.innerHeight,Math.ceil(bottom));"
        "const clipWidth=clipRight-clipLeft;"
        "const clipHeight=clipBottom-clipTop;"
        "if(clipWidth<2||clipHeight<2)return;"
        "const canvasRect=canvas.getBoundingClientRect();"
        "if(!canvasRect||canvasRect.width<1||canvasRect.height<1)return;"
        "const scaleX=canvas.width/canvasRect.width;"
        "const scaleY=canvas.height/canvasRect.height;"
        "const srcX=Math.max(0,Math.floor((clipLeft-canvasRect.left)*scaleX));"
        "const srcY=Math.max(0,Math.floor((clipTop-canvasRect.top)*scaleY));"
        "const srcRight=Math.min(canvas.width,Math.ceil((clipRight-canvasRect.left)*scaleX));"
        "const srcBottom=Math.min(canvas.height,Math.ceil((clipBottom-canvasRect.top)*scaleY));"
        "const srcWidth=srcRight-srcX;"
        "const srcHeight=srcBottom-srcY;"
        "if(srcWidth<1||srcHeight<1)return;"
        "const ctx=getCaptureContext(clipWidth,clipHeight);"
        "if(!ctx)return;"
        "ctx.clearRect(0,0,clipWidth,clipHeight);"
        "ctx.drawImage(canvas,srcX,srcY,srcWidth,srcHeight,0,0,clipWidth,clipHeight);"
        "const image=ctx.getImageData(0,0,clipWidth,clipHeight);"
        "const data=image&&image.data;"
        "if(!data||!data.length)return;"
        "let active=new Map();"
        "for(let y=0;y<clipHeight;y++){"
        "const row=new Map();"
        "let x=0;"
        "while(x<clipWidth){"
        "let alpha=data[(y*clipWidth+x)*4+3];"
        "while(x<clipWidth&&alpha<16){x++;alpha=data[(y*clipWidth+x)*4+3];}"
        "if(x>=clipWidth)break;"
        "const start=x;"
        "alpha=data[(y*clipWidth+x)*4+3];"
        "while(x<clipWidth&&alpha>=16){x++;alpha=data[(y*clipWidth+x)*4+3];}"
        "const runWidth=x-start;"
        "if(runWidth<1)continue;"
        "const key=start+':'+runWidth;"
        "const existing=active.get(key);"
        "if(existing&&existing.y+existing.height===clipTop+y){"
        "existing.height+=1;"
        "row.set(key,existing);"
        "}else{"
        "const rect={x:clipLeft+start,y:clipTop+y,width:runWidth,height:1};"
        "rects.push(rect);"
        "row.set(key,rect);"
        "}"
        "}"
        "active=row;"
        "}"
        "}catch(_){}}"
        "function addVrmModelRect(rects){"
        "try{"
        "const manager=window.vrmManager;"
        "if(!manager||!manager.currentModel||!manager.currentModel.scene||!manager.camera||!window.THREE)return;"
        "const interaction=manager.interaction;"
        "if(interaction&&typeof interaction.checkLocked==='function'&&interaction.checkLocked())return;"
        "if(interaction&&typeof interaction.checkLocked!=='function'&&interaction.isLocked)return;"
        "const canvas=(manager.renderer&&manager.renderer.domElement)||document.getElementById('vrm-canvas');"
        "if(!canvas)return;"
        "const canvasRect=canvas.getBoundingClientRect();"
        "const box=new window.THREE.Box3().setFromObject(manager.currentModel.scene);"
        "if(!Number.isFinite(box.min.x)||!Number.isFinite(box.max.x))return;"
        "const corners=["
        "new window.THREE.Vector3(box.min.x,box.min.y,box.min.z),"
        "new window.THREE.Vector3(box.min.x,box.min.y,box.max.z),"
        "new window.THREE.Vector3(box.min.x,box.max.y,box.min.z),"
        "new window.THREE.Vector3(box.min.x,box.max.y,box.max.z),"
        "new window.THREE.Vector3(box.max.x,box.min.y,box.min.z),"
        "new window.THREE.Vector3(box.max.x,box.min.y,box.max.z),"
        "new window.THREE.Vector3(box.max.x,box.max.y,box.min.z),"
        "new window.THREE.Vector3(box.max.x,box.max.y,box.max.z)];"
        "let left=Infinity,right=-Infinity,top=Infinity,bottom=-Infinity;"
        "for(const corner of corners){"
        "corner.project(manager.camera);"
        "const sx=canvasRect.left+(corner.x*0.5+0.5)*canvasRect.width;"
        "const sy=canvasRect.top+(-corner.y*0.5+0.5)*canvasRect.height;"
        "left=Math.min(left,sx);right=Math.max(right,sx);top=Math.min(top,sy);bottom=Math.max(bottom,sy);"
        "}"
        "if(!Number.isFinite(left)||!Number.isFinite(right)||!Number.isFinite(top)||!Number.isFinite(bottom))return;"
        "addApproxRects(rects,left,top,right,bottom);"
        "}catch(_){}}"
        "function collect(){"
        "scheduled=false;"
        "const uiRects=[];"
        "const avatarRects=[];"
        "const dragRects=[];"
        "const noDragRects=[];"
        "const seen=new Set();"
        "addLive2DModelRect(avatarRects);"
        "addVrmModelRect(avatarRects);"
        "for(const el of document.querySelectorAll('body *')){"
        "if(!el||!visibleAndEnabled(el)||!acceptsPointer(el))continue;"
        "const region=appRegion(el);"
        "if(region!=='drag'&&region!=='no-drag')continue;"
        "const r=el.getBoundingClientRect();"
        "if(region==='drag')addRect(dragRects,r);"
        "if(region==='no-drag')addRect(noDragRects,r);"
        "}"
        "for(const sel of selectors){"
        "for(const el of document.querySelectorAll(sel)){"
        "if(!el||seen.has(el))continue;"
        "seen.add(el);"
        "if(!visibleAndEnabled(el))continue;"
        "if(!acceptsPointer(el)&&!interactiveTag(el))continue;"
        "const r=el.getBoundingClientRect();"
        "if(r.width>=window.innerWidth*0.98&&r.height>=window.innerHeight*0.98&&!interactiveTag(el)&&!looksLikeInteractiveOverlay(el))continue;"
        "addRect(uiRects,r);"
        "}"
        "}"
        "for(const el of document.querySelectorAll(interactiveLeafSelector)){"
        "if(!el||seen.has(el)||!visibleAndEnabled(el)||!acceptsPointer(el))continue;"
        "const r=el.getBoundingClientRect();"
        "if(r.width<2||r.height<2)continue;"
        "if(!isInteractiveLeaf(el))continue;"
        "seen.add(el);"
        "addRect(uiRects,r);"
        "promoteInteractiveAncestors(uiRects,el,seen);"
        "}"
        "for(const el of document.querySelectorAll('body *')){"
        "if(!el||seen.has(el)||!visibleAndEnabled(el))continue;"
        "if(!acceptsPointer(el)&&!interactiveTag(el))continue;"
        "const overlay=looksLikeInteractiveOverlay(el);"
        "if(!overlay)continue;"
        "seen.add(el);"
        "addRect(uiRects,el.getBoundingClientRect());"
        "}"
        "const payload=JSON.stringify(normalizeRects(normalizeRects(uiRects,384).concat(normalizeRects(avatarRects,768)),1152));"
        "if(payload!==last){last=payload;console.log('NEKO_INPUT_REGION:'+payload);}"
        "const dragPayload=JSON.stringify(normalizeRects(dragRects,128));"
        "if(dragPayload!==lastDrag){lastDrag=dragPayload;console.log('NEKO_DRAG_REGION:'+dragPayload);}"
        "const noDragPayload=JSON.stringify(normalizeRects(noDragRects,128));"
        "if(noDragPayload!==lastNoDrag){lastNoDrag=noDragPayload;console.log('NEKO_DRAG_EXCLUSION_REGION:'+noDragPayload);}"
        "}"
        "function schedule(){"
        "if(scheduled)return;"
        "scheduled=true;"
        "if(timer)clearTimeout(timer);"
        "timer=setTimeout(function(){timer=0;requestAnimationFrame(collect);},50);"
        "}"
        "window.addEventListener('resize',schedule,{passive:true});"
        "window.addEventListener('load',schedule,{passive:true});"
        "document.addEventListener('readystatechange',schedule,{passive:true});"
        "document.addEventListener('DOMContentLoaded',schedule,{passive:true});"
        "window.addEventListener('live2d-floating-buttons-ready',schedule,{passive:true});"
        "window.addEventListener('live2d-agent-popup-opening',schedule,{passive:true});"
        "window.addEventListener('live2d-agent-popup-closed',schedule,{passive:true});"
        "new MutationObserver(schedule).observe(document.documentElement,{subtree:true,childList:true,attributes:true,attributeFilter:['style','class','hidden']});"
        "setInterval(schedule,1500);"
        "setInterval(function(){try{if(window.live2dManager&&window.live2dManager.currentModel&&!window.live2dManager.isLocked)schedule();}catch(_){}} ,250);"
        "setTimeout(schedule,0);"
        "setTimeout(schedule,300);"
        "setTimeout(schedule,1000);"
        "})();";
    neko_cef_bridge_browser_execute_javascript(
        state->browser, k_region_script, "neko://input-region-bridge", 1);
  }
}

static void on_load_error(void* user_data,
                          int error_code,
                          const char* error_text,
                          const char* failed_url) {
  (void)user_data;
  fprintf(stderr,
          "NEKO_CEF_STANDALONE load_error error_code=%d error_text=%s failed_url=%s\n",
          error_code,
          error_text ? error_text : "<null>",
          failed_url ? failed_url : "<null>");
  fflush(stderr);
}

static void on_console(void* user_data,
                       int level,
                       const char* source,
                       int line,
                       const char* message) {
  (void)user_data;
  if (message && strncmp(message, "NEKO_FRONTEND_IPC_B64:", 22) == 0) {
    size_t payload_len = strlen(message + 22);
    char* details = (char*)malloc(payload_len + 13);
    if (!details) {
      return;
    }
    snprintf(details, payload_len + 13, "payload_b64=%s", message + 22);
    emit_event("frontend_ipc", details);
    free(details);
    return;
  }
  if (message && strncmp(message, "NEKO_INPUT_REGION:", 18) == 0) {
    size_t rects_len = strlen(message + 18);
    char* details = (char*)malloc(rects_len + 7);
    if (!details) {
      return;
    }
    snprintf(details, rects_len + 7, "rects=%s", message + 18);
    emit_event("input_region", details);
    free(details);
    return;
  }
  if (message && strncmp(message, "NEKO_DRAG_REGION:", 17) == 0) {
    size_t rects_len = strlen(message + 17);
    char* details = (char*)malloc(rects_len + 7);
    if (!details) {
      return;
    }
    snprintf(details, rects_len + 7, "rects=%s", message + 17);
    emit_event("drag_region", details);
    free(details);
    return;
  }
  if (message && strncmp(message, "NEKO_DRAG_EXCLUSION_REGION:", 27) == 0) {
    size_t rects_len = strlen(message + 27);
    char* details = (char*)malloc(rects_len + 7);
    if (!details) {
      return;
    }
    snprintf(details, rects_len + 7, "rects=%s", message + 27);
    emit_event("drag_exclusion_region", details);
    free(details);
    return;
  }
  if (!env_flag_enabled("NEKO_CEF_VERBOSE_CONSOLE")) {
    return;
  }
  fprintf(stderr,
          "NEKO_CEF_STANDALONE console level=%d source=%s:%d message=%s\n",
          level,
          source ? source : "<null>",
          line,
          message ? message : "<null>");
  fflush(stderr);
}

static void maybe_dump_frame(neko_cef_standalone_state_t* state,
                             const void* buffer,
                             int width,
                             int height) {
  FILE* file;
  size_t size;
  char tmp_path[1200];

  if (!state) {
    fprintf(stderr, "NEKO_CEF_STANDALONE dump skipped: state is null\n");
    fflush(stderr);
    return;
  }
  if (!state->frame_dump_path[0]) {
    fprintf(stderr, "NEKO_CEF_STANDALONE dump skipped: frame_dump_path is empty\n");
    fflush(stderr);
    return;
  }
  if (!buffer) {
    fprintf(stderr,
            "NEKO_CEF_STANDALONE dump skipped: buffer is null for %s\n",
            state->frame_dump_path);
    fflush(stderr);
    return;
  }
  if (width <= 0 || height <= 0) {
    fprintf(stderr,
            "NEKO_CEF_STANDALONE dump skipped: invalid size %dx%d for %s\n",
            width,
            height,
            state->frame_dump_path);
    fflush(stderr);
    return;
  }

  snprintf(tmp_path, sizeof(tmp_path), "%s.tmp", state->frame_dump_path);

  file = fopen(tmp_path, "wb");
  if (!file) {
    fprintf(stderr,
            "NEKO_CEF_STANDALONE failed to open frame dump path: %s\n",
            tmp_path);
    fflush(stderr);
    return;
  }

  size = (size_t)width * (size_t)height * 4u;
  if (fwrite(buffer, 1, size, file) != size) {
    fprintf(stderr,
            "NEKO_CEF_STANDALONE failed writing frame dump: %s\n",
            tmp_path);
    fflush(stderr);
    fclose(file);
    return;
  }

  fclose(file);
  if (rename(tmp_path, state->frame_dump_path) != 0) {
    fprintf(stderr,
            "NEKO_CEF_STANDALONE failed renaming frame dump %s -> %s\n",
            tmp_path,
            state->frame_dump_path);
    fflush(stderr);
    return;
  }
  if (!state->frame_dump_log_emitted) {
    state->frame_dump_log_emitted = 1;
    fprintf(stderr,
            "NEKO_CEF_STANDALONE dumping frames to %s (%dx%d bgra)\n",
            state->frame_dump_path,
            width,
            height);
    fflush(stderr);
  }
}

static void maybe_write_shared_frame(neko_cef_standalone_state_t* state,
                                     const void* buffer,
                                     int width,
                                     int height) {
  neko_shared_frame_root_header_t* root;
  neko_shared_frame_slot_header_t* header;
  size_t data_len;
  size_t slot_size;
  size_t slot_offset;
  uint8_t* payload;
  uint32_t slot_index;

  if (!state || !state->shared_frame_map || state->shared_frame_map_len < sizeof(*header) || !buffer ||
      width <= 0 || height <= 0) {
    return;
  }

  data_len = (size_t)width * (size_t)height * 4u;
  slot_size = sizeof(*header) + data_len;
  if (sizeof(*root) + slot_size * NEKO_SHARED_FRAME_SLOT_COUNT > state->shared_frame_map_len) {
    fprintf(stderr,
            "NEKO_CEF_STANDALONE shared frame too small: map=%zu need=%zu\n",
            state->shared_frame_map_len,
            sizeof(*root) + slot_size * NEKO_SHARED_FRAME_SLOT_COUNT);
    fflush(stderr);
    return;
  }

  root = (neko_shared_frame_root_header_t*)state->shared_frame_map;
  root->magic = NEKO_SHARED_FRAME_MAGIC;
  root->version = NEKO_SHARED_FRAME_VERSION;
  root->slot_count = NEKO_SHARED_FRAME_SLOT_COUNT;
  slot_index = (uint32_t)(state->frame_count % (int)NEKO_SHARED_FRAME_SLOT_COUNT);
  slot_offset = sizeof(*root) + slot_size * slot_index;
  header = (neko_shared_frame_slot_header_t*)((uint8_t*)state->shared_frame_map + slot_offset);
  payload = (uint8_t*)header + sizeof(*header);

  __sync_add_and_fetch(&header->seq, 1);
  __sync_synchronize();
  header->frame = (uint32_t)(state ? state->frame_count : 0);
  header->width = (uint32_t)width;
  header->height = (uint32_t)height;
  header->stride = (uint32_t)width * 4u;
  header->data_len = (uint32_t)data_len;
  memcpy(payload, buffer, data_len);
  __sync_synchronize();
  __sync_add_and_fetch(&header->seq, 1);
  __sync_synchronize();
  root->latest_slot = slot_index;
}

static void on_paint(void* user_data,
                     int element_type,
                     const void* buffer,
                     int width,
                     int height) {
  char details[160];
  neko_cef_standalone_state_t* state = (neko_cef_standalone_state_t*)user_data;

  if (state) {
    state->frame_count += 1;
  }

  snprintf(details,
           sizeof(details),
           "element_type=%d width=%d height=%d frame=%d",
           element_type,
           width,
           height,
           state ? state->frame_count : 0);
  if (state && state->frame_count <= 2) {
    fprintf(stderr,
            "NEKO_CEF_STANDALONE on_paint frame=%d element_type=%d size=%dx%d buffer=%p path=%s\n",
            state->frame_count,
            element_type,
            width,
            height,
            buffer,
            state->frame_dump_path[0] ? state->frame_dump_path : "<null>");
    fflush(stderr);
  }
  maybe_dump_frame(state, buffer, width, height);
  maybe_write_shared_frame(state, buffer, width, height);
  if (element_type != 0) {
    return;
  }
  emit_event("paint", details);
}

int main(int argc, char** argv) {
  char error[1024] = {0};
  neko_cef_runtime_settings_t settings;
  neko_cef_runtime_t* runtime;
  neko_cef_browser_t* browser = NULL;
  neko_cef_browser_config_t browser_config;
  neko_cef_browser_callbacks_t callbacks;
  neko_cef_standalone_state_t state;
  char command_buffer[NEKO_COMMAND_BUFFER_CAPACITY];
  size_t command_buffer_used;
  int code;
  int should_shutdown = 0;
  int loop_sleep_ms = 4;

  memset(&settings, 0, sizeof(settings));
  memset(&state, 0, sizeof(state));
  memset(command_buffer, 0, sizeof(command_buffer));
  command_buffer_used = 0;
  settings.browser_subprocess_path = env_or_null("NEKO_CEF_BROWSER_SUBPROCESS_PATH");
  settings.resources_dir_path = env_or_null("NEKO_CEF_RESOURCES_DIR");
  settings.locales_dir_path = env_or_null("NEKO_CEF_LOCALES_DIR");
  settings.locale = env_or_null("NEKO_CEF_LOCALE");
  settings.cache_path = env_or_null("NEKO_CEF_CACHE_PATH");
  settings.root_cache_path = env_or_null("NEKO_CEF_ROOT_CACHE_PATH");
  loop_sleep_ms = env_int_or_default("NEKO_CEF_HELPER_LOOP_SLEEP_MS", 4);
  state.shared_frame_fd = env_int_or_default("NEKO_CEF_SHARED_FRAME_FD", -1);
  state.shared_frame_map_len = env_size_or_zero("NEKO_CEF_SHARED_FRAME_SIZE");
  {
    const char* frame_dump_path = env_or_null("NEKO_CEF_FRAME_DUMP_PATH");
    if (frame_dump_path) {
      snprintf(state.frame_dump_path, sizeof(state.frame_dump_path), "%s", frame_dump_path);
    }
  }
  if (state.shared_frame_fd >= 0 && state.shared_frame_map_len > 0) {
    state.shared_frame_map =
        mmap(NULL, state.shared_frame_map_len, PROT_READ | PROT_WRITE, MAP_SHARED, state.shared_frame_fd, 0);
    if (state.shared_frame_map == MAP_FAILED) {
      fprintf(stderr,
              "NEKO_CEF_STANDALONE failed to mmap shared frame bridge fd=%d size=%zu: %s\n",
              state.shared_frame_fd,
              state.shared_frame_map_len,
              strerror(errno));
      fflush(stderr);
      state.shared_frame_map = NULL;
      state.shared_frame_map_len = 0;
    } else {
      memset(state.shared_frame_map, 0, state.shared_frame_map_len);
      fprintf(stderr,
              "NEKO_CEF_STANDALONE using shared frame bridge fd=%d size=%zu\n",
              state.shared_frame_fd,
              state.shared_frame_map_len);
      fflush(stderr);
    }
  }
  settings.no_sandbox = 1;
  settings.multi_threaded_message_loop = 0;
  settings.windowless_rendering_enabled = 1;
  settings.external_message_pump = 0;
  settings.remote_debugging_port = 0;
  settings.use_app = 1;

  fprintf(stderr,
          "NEKO_CEF_STANDALONE main argc=%d argv0=%s cwd settings: subprocess=%s resources=%s locales=%s locale=%s frame_dump=%s shared_fd=%d shared_size=%zu\n",
          argc,
          (argc > 0 && argv && argv[0]) ? argv[0] : "<null>",
          settings.browser_subprocess_path ? settings.browser_subprocess_path : "<null>",
          settings.resources_dir_path ? settings.resources_dir_path : "<null>",
          settings.locales_dir_path ? settings.locales_dir_path : "<null>",
          settings.locale ? settings.locale : "<null>",
          state.frame_dump_path[0] ? state.frame_dump_path : "<null>",
          state.shared_frame_fd,
          state.shared_frame_map_len);
  fflush(stderr);
  emit_event("startup", "");
  configure_nonblocking_stdin();

  code = neko_cef_bridge_execute_process(argc, argv, settings.use_app);
  if (code >= 0) {
    char details[64];
    snprintf(details, sizeof(details), "code=%d", code);
    emit_event("subprocess_exit", details);
    fprintf(stderr, "NEKO_CEF_STANDALONE subprocess exit code=%d\n", code);
    fflush(stderr);
    return code;
  }

  runtime = neko_cef_bridge_initialize(argc, argv, &settings, error, sizeof(error));
  if (!runtime) {
    emit_event("initialize_failed", error[0] ? error : "message=<empty>");
    fprintf(stderr, "NEKO_CEF_STANDALONE initialize failed: %s\n", error[0] ? error : "<empty>");
    fflush(stderr);
    return 2;
  }

  emit_event("initialize_ok", "");
  fprintf(stderr, "NEKO_CEF_STANDALONE initialize ok\n");
  fflush(stderr);

  if (argc > 1 && argv[1] && argv[1][0]) {
    memset(&browser_config, 0, sizeof(browser_config));
    memset(&callbacks, 0, sizeof(callbacks));
    browser_config.url = argv[1];
    browser_config.window_name = "neko-cef-standalone";
    browser_config.width = env_int_or_default("NEKO_CEF_HELPER_WIDTH", 1920);
    browser_config.height = env_int_or_default("NEKO_CEF_HELPER_HEIGHT", 1080);
    browser_config.frame_rate = env_int_or_default("NEKO_CEF_HELPER_FRAME_RATE", 30);
    browser_config.transparent_painting = env_flag_enabled("NEKO_CEF_HELPER_TRANSPARENT") ? 1 : 0;

    callbacks.on_after_created = on_after_created;
    callbacks.on_before_close = on_before_close;
    callbacks.on_load_start = on_load_start;
    callbacks.on_load_end = on_load_end;
    callbacks.on_load_error = on_load_error;
    callbacks.on_console = on_console;
    callbacks.on_paint = on_paint;

    browser =
        neko_cef_bridge_create_browser(runtime, &browser_config, &callbacks, &state, error, sizeof(error));
    if (!browser) {
      emit_event("create_browser_failed", error[0] ? error : "message=<empty>");
      fprintf(stderr,
              "NEKO_CEF_STANDALONE create_browser failed: %s\n",
              error[0] ? error : "<empty>");
      fflush(stderr);
      neko_cef_bridge_shutdown(runtime);
      return 3;
    }

    emit_event("create_browser_ok", "url_set=1");
    state.browser = browser;
    fprintf(stderr, "NEKO_CEF_STANDALONE create_browser ok url=%s\n", browser_config.url);
    fflush(stderr);

    while (!should_shutdown) {
      should_shutdown = process_stdin_commands(browser, command_buffer, &command_buffer_used);
      cef_do_message_loop_work();
      sleep_ms(loop_sleep_ms);
    }

    neko_cef_bridge_browser_close(browser);
    for (int i = 0; i < 60; ++i) {
      cef_do_message_loop_work();
      sleep_ms(loop_sleep_ms);
    }
    neko_cef_bridge_browser_release(browser);
    emit_event("browser_released", "");
    fprintf(stderr, "NEKO_CEF_STANDALONE browser released\n");
    fflush(stderr);
  }

  neko_cef_bridge_shutdown(runtime);
  if (state.shared_frame_map && state.shared_frame_map_len > 0) {
    munmap(state.shared_frame_map, state.shared_frame_map_len);
  }
  emit_event("shutdown_ok", "");
  fprintf(stderr, "NEKO_CEF_STANDALONE shutdown ok\n");
  fflush(stderr);
  return 0;
}
