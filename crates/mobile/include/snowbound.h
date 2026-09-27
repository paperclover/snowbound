// The C surface of crates/mobile; see its lib.rs for each call's contract.
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct Section Section;
typedef struct View View;

char *sb_notebook(const char *path);

Section *sb_section_open(const char *path);
size_t sb_section_count(const Section *section);
const char *sb_section_title(const Section *section, size_t index);
uint32_t sb_section_level(const Section *section, size_t index);
void sb_section_free(Section *section);

View *sb_view_new(void *layer, const Section *section, size_t index, float width, float height, float scale);
void sb_view_free(View *view);
void sb_view_resize(View *view, float width, float height, float scale);
bool sb_view_render(View *view);
bool sb_view_frame_pending(const View *view);
void sb_view_set_dark(View *view, bool dark);
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
bool sb_view_toggle(View *view, uint8_t toggle);
char *sb_view_edit(View *view);
void sb_string_free(char *text);

uint32_t sb_text_length(const View *view);
char *sb_text(const View *view, uint32_t start, uint32_t end);
void sb_selection(const View *view, uint32_t range[2]);
bool sb_select(View *view, uint32_t start, uint32_t end);
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
