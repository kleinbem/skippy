struct ggml_tensor {};
struct model_type {
  ggml_tensor *tok_embd;
};

ggml_tensor *ggml_get_rows(void *, ggml_tensor *, ggml_tensor *);

// A token table returned by a helper may be this model's or another
// context's, so the gather proves no embedding owner of this graph.
struct model_helper_table {
  static ggml_tensor *get_tok_embd(const model_type &model);

  struct graph {
    graph(const model_type &model);

    ggml_tensor *block(ggml_tensor *cur, int) { return cur; }
    void ggml_build_forward_expand(void *, ggml_tensor *);

    int n_layer = 4;
    ggml_tensor *inp_tokens;
    void *ctx0;
    void *gf;
  };
};

model_helper_table::graph::graph(const model_type &model) {
  ggml_tensor *cur;
  ggml_tensor *inpL;

  inpL = ggml_get_rows(ctx0, get_tok_embd(model), inp_tokens);

  for (int il = 0; il < n_layer; ++il) {
    cur = block(inpL, il);
    inpL = cur;
  }

  ggml_build_forward_expand(gf, inpL);
}
