// The C surface of crates/mobile; see its lib.rs and library.rs for each call's contract.
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct Library Library;
typedef struct Share Share;
typedef struct Section Section;
typedef struct View View;

void sb_string_free(char *text);

typedef void (*sb_coordinator)(const char *path, bool write, void (*body)(void *), void *context);
void sb_set_coordinator(sb_coordinator coordinator);
void sb_set_sync_wake(void (*wake)(void));
char *sb_tags(void);
char *sb_pens(void);
bool sb_tag_icon(uint16_t shape, bool checked, uint32_t pixels, uint8_t *rgba);

const Library *sb_library_open(const char *path, const char *cache, bool local, char **error);
const Library *sb_library_server(const char *address, const char *share, const char *user, const char *password,
                                 const char *domain, const char *root, const char *cache, char **error);
char *sb_library_sections(const Library *library);
char *sb_library_new_section(const Library *library, const char *folder, const char *name, const char *author,
                             const char *date, const char *time);
char *sb_library_search(const Library *library, const Section *open, const char *path, const char *query);
char *sb_library_tagged(const Library *library, const Section *open, const char *path);
char *sb_library_sync_status(const Library *library);
void sb_library_set_offline(const Library *library, bool offline);
void sb_library_touched(const Library *library, const char *path);
void sb_library_sync_now(const Library *library);
void sb_library_free(const Library *library);

Share *sb_share_connect(const char *address, const char *share, const char *user, const char *password,
                        const char *domain, char **error);
char *sb_share_list(const Share *share, const char *path);
void sb_share_free(Share *share);

typedef void (*sb_wake)(uintptr_t token);
Section *sb_section_open(const Library *library, const char *path, const char *author, sb_wake wake,
                         uintptr_t token, char **error);
char *sb_section_pages(const Section *section);
uint32_t sb_section_poll(const Section *section, uint8_t *status);
char *sb_section_new_page(const Section *section, const char *parent, const char *date, const char *time);
bool sb_section_delete_page(const Section *section, const char *id, const char *date, const char *time);
bool sb_section_flush(const Section *section, double seconds);
void sb_section_wake(const Section *section);
void sb_section_free(Section *section);

View *sb_view_new(void *layer, const Section *section, const char *id, float width, float height, float scale);
void sb_view_free(View *view);
void sb_view_resize(View *view, float width, float height, float scale);
bool sb_view_render(View *view);
bool sb_view_frame_pending(const View *view);
void sb_view_set_dark(View *view, bool dark);
bool sb_view_read_only(const View *view);
bool sb_view_reload(View *view, bool discard);
void sb_view_focus(View *view, bool focused);
void sb_view_content(View *view, float bounds[4]);
bool sb_view_block(const View *view, float x, float y, float rect[4]);
void sb_view_set_transform(View *view, float zoom, float x, float y);
uint8_t sb_view_target(const View *view, float x, float y);
bool sb_view_press(View *view, float x, float y);
bool sb_view_drag(View *view, float x, float y);
bool sb_view_release(View *view);
bool sb_view_undo(View *view, bool redo);
bool sb_view_can_undo(const View *view, bool redo);
uint64_t sb_view_format(const View *view);
bool sb_view_apply(View *view, uint8_t command);
char *sb_view_title(const View *view);
bool sb_view_focus_title(View *view);
bool sb_view_find(View *view, const char *query);
bool sb_view_select_paragraph(View *view, const char *id);
char *sb_view_copy(View *view, bool cut);
void sb_view_insert_space(View *view);
void sb_view_set_tool(View *view, uint8_t tool, uint8_t pen);
bool sb_view_ink_selection(const View *view, float rect[4]);
char *sb_view_paper(const View *view);
bool sb_view_set_paper(View *view, int16_t red, uint8_t green, uint8_t blue, int8_t ruled);
bool sb_view_set_art(View *view, const char *name);
char *sb_view_page_text(const View *view);
int8_t sb_view_date_request(View *view, int64_t *seconds);
bool sb_view_change_date(View *view, int64_t seconds, const char *date, const char *time);
bool sb_view_insert_picture(View *view, const uint8_t *bytes, size_t length, float width, float height);

uint32_t sb_text_length(const View *view);
char *sb_text(const View *view, uint32_t start, uint32_t end);
void sb_selection(const View *view, uint32_t range[2]);
bool sb_select(View *view, uint32_t start, uint32_t end);
bool sb_select_more(View *view);
bool sb_marked(const View *view, uint32_t range[2]);
bool sb_set_marked(View *view, const char *text, uint32_t selected_start, uint32_t selected_end);
void sb_unmark(View *view);
bool sb_insert(View *view, const char *text);
bool sb_replace(View *view, uint32_t start, uint32_t end, const char *text);
bool sb_paste(View *view, const char *text, const char *language);
bool sb_delete_backward(View *view);
bool sb_caret_rect(const View *view, uint32_t offset, float rect[4]);
size_t sb_range_rects(const View *view, uint32_t start, uint32_t end, float (*rects)[4], size_t capacity);
uint32_t sb_closest(const View *view, float x, float y);
