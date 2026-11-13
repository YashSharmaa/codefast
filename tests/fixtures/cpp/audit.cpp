// Cases found by the 2026-11-13 audit.
#include "llvm/Support/Error.h"
using namespace llvm;

// Preprocessor directives inside a constructor's initialiser list: the body still ends where
// it ends, and what follows is still defined.
class Selector {
public:
  Selector(const Target &TM, const Subtarget &STI);

private:
  const Target &TM;
  const Subtarget &STI;
};

Selector::Selector(const Target &TM, const Subtarget &STI)
    : TM(TM), STI(STI),
#define GET_PREDICATES_INIT
#include "GenSelector.inc"
#undef GET_PREDICATES_INIT
{
}

struct StoreInfo {
  int Offset = 0;
};

static bool afterSelector(StoreInfo &S) { return S.Offset == 0; }

// A macro call where an enumerator is expected.
enum class Access : unsigned {
  Read = 1,
  Write = 2,
  MARK_AS_BITMASK_ENUM(/*LargestValue=*/Write)
};

static Access afterEnum(unsigned Bits) { return static_cast<Access>(Bits); }

// A data member that shares its name with an enumerator is still reached through `this`.
class Pointer {
  enum class Storage { Int, Fn };
  struct FnPtr {
    const Decl *Func;
  };
  bool isWeak() const { return Kind == Storage::Fn && Fn.Func->isWeak(); }
  Storage Kind;
  union {
    long Int;
    FnPtr Fn;
  };
};

// `.` on a local smart pointer or `Expected` is a member of the wrapper class, `->` reaches
// the pointee.
Error useWrappers() {
  Expected<StoreInfo> E = loadInfo();
  if (!E)
    return E.takeError();
  E->Offset = 1;
  ErrorOr<Selector> S = openSelector();
  if (std::error_code EC = S.getError())
    return errorCodeToError(EC);
  std::optional<StoreInfo> O;
  O.reset();
  return Error::success();
}

// A definition qualified by a namespace this file never opens is a function of that
// namespace, not a member function of a class.
detail::Holder llvm::detail::takeHolder(Error Err) { return wrap(Err); }
std::string arm::getTargetCPU(StringRef Arch) { return lookup(Arch); }
// A class in lower case keeps its member functions: it has a constructor here.
line_iterator::line_iterator(const MemoryBuffer &B) : Buffer(&B) {}
void line_iterator::advance() { skip(); }
